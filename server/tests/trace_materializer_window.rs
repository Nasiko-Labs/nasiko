//! The trace materializer's cursor and windowing: each pass searches one
//! bounded window, moves the cursor whether or not it found anything, and a
//! rescan that changes nothing leaves `materialized_at` where it was.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test trace_materializer_window -- --test-threads=1

#[allow(dead_code)]
mod common;

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use nasiko_observability::{
    AgentFinOps, AgentStats, CostBreakdown, CostRequest, NoSessionIdResolver, ObservabilityError,
    ObservabilityProvider, Session, SessionDetails, SpanDetails, TraceDetails, TraceUsageRow,
};
use nasiko_server::observability::trace_materializer::materialize_once;
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

const OVERLAP: Duration = Duration::from_secs(180);
const MAX_WINDOW: Duration = Duration::from_secs(600);

/// Tempo stand-in: returns `traces` from every search, answers each fetch with
/// `usage` (or fails it), and records every search window it was asked for.
#[derive(Default)]
struct FakeTempo {
    traces: Vec<String>,
    usage: Mutex<Option<TraceUsageRow>>,
    searches: Mutex<Vec<(DateTime<Utc>, DateTime<Utc>)>>,
}

#[async_trait]
impl ObservabilityProvider for FakeTempo {
    async fn search_user_traces(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        _limit: usize,
    ) -> Result<Vec<String>, ObservabilityError> {
        self.searches.lock().unwrap().push((start, end));
        Ok(self.traces.clone())
    }

    async fn extract_trace_usage(
        &self,
        trace_id: &str,
    ) -> Result<Vec<TraceUsageRow>, ObservabilityError> {
        match self.usage.lock().unwrap().clone() {
            Some(row) if row.trace_id == trace_id => Ok(vec![row]),
            // Another test's flow that happens to fall in the window.
            Some(_) => Ok(Vec::new()),
            None => Err(ObservabilityError::TempoError("fetch failed".into())),
        }
    }

    async fn sessions_for_agent(
        &self,
        _: &str,
        _: DateTime<Utc>,
        _: DateTime<Utc>,
    ) -> Result<Vec<Session>, ObservabilityError> {
        unreachable!("the materializer never lists sessions")
    }
    async fn get_session(
        &self,
        _: &str,
        _: DateTime<Utc>,
        _: DateTime<Utc>,
    ) -> Result<SessionDetails, ObservabilityError> {
        unreachable!("the materializer never reads a session")
    }
    async fn get_trace(&self, _: &str) -> Result<TraceDetails, ObservabilityError> {
        unreachable!("the materializer reads traces through extract_trace_usage")
    }
    async fn get_span(&self, _: &str, _: &str) -> Result<SpanDetails, ObservabilityError> {
        unreachable!("the materializer never reads a span")
    }
    async fn agent_stats(
        &self,
        _: &str,
        _: DateTime<Utc>,
        _: DateTime<Utc>,
    ) -> Result<AgentStats, ObservabilityError> {
        unreachable!("the materializer never reads agent stats")
    }
    async fn agent_finops(
        &self,
        _: &str,
        _: DateTime<Utc>,
        _: DateTime<Utc>,
    ) -> Result<AgentFinOps, ObservabilityError> {
        unreachable!("the materializer never reads agent FinOps")
    }
    async fn count_user_traces(
        &self,
        _: &str,
        _: DateTime<Utc>,
        _: DateTime<Utc>,
    ) -> Result<usize, ObservabilityError> {
        unreachable!("the materializer never counts traces")
    }
    async fn query_logs(
        &self,
        _: &str,
        _: Option<DateTime<Utc>>,
        _: Option<DateTime<Utc>>,
        _: usize,
    ) -> Result<Vec<(DateTime<Utc>, String)>, ObservabilityError> {
        unreachable!("the materializer never reads logs")
    }
    async fn cost(&self, _: CostRequest<'_>) -> CostBreakdown {
        unreachable!("rows arrive already priced")
    }
}

fn usage(trace_id: &str, input_tokens: u64) -> TraceUsageRow {
    TraceUsageRow {
        trace_id: trace_id.into(),
        agent_name: "window-test-agent".into(),
        agent_uuid: None,
        session_id: None,
        model: Some("gpt-4o".into()),
        provider: Some("openai".into()),
        input_tokens,
        output_tokens: 10,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        tool_call_count: 0,
        cost_estimated: false,
        cost_usd: 0.01,
        prompt_cost_usd: 0.008,
        completion_cost_usd: 0.002,
        latency_ms: Some(50),
        started_at: Utc::now() - chrono::Duration::minutes(2),
        error_count: 0,
        tool_calls: Vec::new(),
    }
}

async fn set_cursor(db: &PgPool, at: DateTime<Utc>) {
    sqlx::query("UPDATE trace_usage_cursor SET high_water = $1 WHERE id = 1")
        .bind(at)
        .execute(db)
        .await
        .unwrap();
}

async fn cursor(db: &PgPool) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT high_water FROM trace_usage_cursor WHERE id = 1")
        .fetch_one(db)
        .await
        .unwrap()
}

async fn pass(db: &PgPool, tempo: &FakeTempo) {
    materialize_once(db, tempo, &NoSessionIdResolver, OVERLAP, MAX_WINDOW, 8)
        .await
        .unwrap();
}

async fn materialized_at(db: &PgPool, trace_id: &str) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT materialized_at FROM trace_usage WHERE trace_id = $1")
        .bind(trace_id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_cursor_from_2020_searches_one_window_an_hour_back_and_advances_when_empty() {
    let server = common::TestServer::start().await;
    set_cursor(
        &server.db,
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )
    .await;
    let tempo = FakeTempo::default();

    let before = Utc::now();
    pass(&server.db, &tempo).await;

    let searches = tempo.searches.lock().unwrap().clone();
    assert_eq!(
        searches.len(),
        1,
        "one window, not one search per week since 2020"
    );
    let (start, end) = searches[0];
    assert!(start >= before - chrono::Duration::hours(1) - chrono::Duration::seconds(5));
    assert_eq!(end - start, chrono::Duration::seconds(600));
    assert_eq!(
        cursor(&server.db).await,
        end,
        "an empty window still moves the cursor"
    );
}

#[tokio::test]
#[serial]
async fn a_failed_fetch_does_not_hold_the_cursor_back() {
    let server = common::TestServer::start().await;
    let start_at = Utc::now() - chrono::Duration::minutes(5);
    set_cursor(&server.db, start_at).await;
    let tempo = FakeTempo {
        traces: vec![Uuid::new_v4().simple().to_string()],
        ..FakeTempo::default()
    };

    pass(&server.db, &tempo).await;

    assert!(cursor(&server.db).await > start_at);
}

#[tokio::test]
#[serial]
async fn a_rescan_stamps_materialized_at_only_when_the_row_changed() {
    let server = common::TestServer::start().await;
    let trace_id = Uuid::new_v4().simple().to_string();
    let tempo = FakeTempo {
        traces: vec![trace_id.clone()],
        usage: Mutex::new(Some(usage(&trace_id, 100))),
        ..FakeTempo::default()
    };
    set_cursor(&server.db, Utc::now()).await;
    pass(&server.db, &tempo).await;

    // Age the stamp so a re-stamp is unmistakable.
    let aged = Utc::now() - chrono::Duration::hours(1);
    sqlx::query("UPDATE trace_usage SET materialized_at = $2 WHERE trace_id = $1")
        .bind(&trace_id)
        .bind(aged)
        .execute(&server.db)
        .await
        .unwrap();

    pass(&server.db, &tempo).await;
    let unchanged = materialized_at(&server.db, &trace_id).await;
    assert!(
        (unchanged - aged).num_milliseconds().abs() < 1,
        "an identical rescan must not look like a new model call"
    );

    *tempo.usage.lock().unwrap() = Some(usage(&trace_id, 150));
    pass(&server.db, &tempo).await;
    assert!(
        materialized_at(&server.db, &trace_id).await > aged + chrono::Duration::minutes(30),
        "a rescan that changed the totals must reach the budget ledger"
    );
}

async fn insert_user(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $1 || '@test.local', false) \
         RETURNING id",
    )
    .bind(username)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn insert_agent(db: &PgPool, name: &str, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn materialize_row(db: &PgPool, row: TraceUsageRow) -> Option<Uuid> {
    let trace_id = row.trace_id.clone();
    let tempo = FakeTempo {
        traces: vec![trace_id.clone()],
        usage: Mutex::new(Some(row)),
        ..FakeTempo::default()
    };
    set_cursor(db, Utc::now()).await;
    pass(db, &tempo).await;
    sqlx::query_scalar("SELECT agent_id FROM trace_usage WHERE trace_id = $1")
        .bind(&trace_id)
        .fetch_one(db)
        .await
        .unwrap()
}

/// Two users each own an agent named `meeting-notes-processor`. A call the spans vouch for by
/// UUID is booked to that agent; one known only by its shared name is booked to neither, never
/// to whichever row the database returned first (bug found 2026-11-18).
#[tokio::test]
#[serial]
async fn a_name_two_agents_share_resolves_by_uuid_or_not_at_all() {
    let server = common::TestServer::start().await;
    let name = format!("shared-name-{}", Uuid::new_v4().simple());
    let alice = insert_user(&server.db, &format!("alice-{}", Uuid::new_v4().simple())).await;
    let bob = insert_user(&server.db, &format!("bob-{}", Uuid::new_v4().simple())).await;
    let alices = insert_agent(&server.db, &name, alice).await;
    let _bobs = insert_agent(&server.db, &name, bob).await;

    let vouched = TraceUsageRow {
        agent_name: name.clone(),
        agent_uuid: Some(alices),
        ..usage(&Uuid::new_v4().simple().to_string(), 100)
    };
    assert_eq!(materialize_row(&server.db, vouched).await, Some(alices));

    let unvouched = TraceUsageRow {
        agent_name: name.clone(),
        ..usage(&Uuid::new_v4().simple().to_string(), 100)
    };
    assert_eq!(
        materialize_row(&server.db, unvouched).await,
        None,
        "an ambiguous name must not be pinned to an arbitrary owner's agent"
    );
}

/// A name only one live agent has resolves by name, as before.
#[tokio::test]
#[serial]
async fn a_unique_name_still_resolves_without_a_uuid() {
    let server = common::TestServer::start().await;
    let name = format!("unique-name-{}", Uuid::new_v4().simple());
    let owner = insert_user(&server.db, &format!("owner-{}", Uuid::new_v4().simple())).await;
    let agent = insert_agent(&server.db, &name, owner).await;

    let row = TraceUsageRow {
        agent_name: name,
        ..usage(&Uuid::new_v4().simple().to_string(), 100)
    };
    assert_eq!(materialize_row(&server.db, row).await, Some(agent));
}
