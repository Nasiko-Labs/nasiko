/// Integration tests for the MAF worker: concurrency, and which queued messages it
/// runs, drops or leaves alone.
///
/// Requires: `just infra`
/// Run with:  `cargo test -p nasiko-server --test maf_worker -- --test-threads=1`
///
/// The worker's stream key and consumer group are fixed, so these tests point it at
/// a Redis database index of their own (`MAF_WORKER_TEST_REDIS_URL`, default db 15)
/// and clear the stream there first. On the shared default index they would read,
/// and acknowledge, whatever a local dev server left queued.
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nasiko_orchestrator::maf::types::{ExecutionResult, MafDefinition, StepOutcome};
use nasiko_orchestrator::maf::{
    LlmConfig, MafExecutor, MafResume, MafRunContext, STREAM_KEY, WorkerConfig, start_worker,
};
use serial_test::serial;
use uuid::Uuid;

const GROUP_NAME: &str = "maf-workers";
/// How long each fake execution holds its slot.
const RUN_TIME: Duration = Duration::from_millis(300);

fn test_redis_url() -> String {
    std::env::var("MAF_WORKER_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6379/15".into())
}

/// Counts runs and resumes, how many are inside the executor at once, and the most
/// there ever were.
#[derive(Default)]
struct CountingExecutor {
    runs: AtomicUsize,
    resumes: AtomicUsize,
    running: AtomicUsize,
    most_at_once: AtomicUsize,
}

impl CountingExecutor {
    async fn hold_a_slot(&self) -> Result<StepOutcome, String> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_at_once.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(RUN_TIME).await;
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(StepOutcome::Completed(ExecutionResult {
            output: "done".into(),
            step_results: vec![],
            tokens_used: 0,
            cost_usd: 0.0,
        }))
    }
}

#[async_trait]
impl MafExecutor for CountingExecutor {
    async fn run(
        &self,
        _ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _content: Option<&str>,
    ) -> Result<StepOutcome, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.hold_a_slot().await
    }

    async fn resume(
        &self,
        _ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _resume: MafResume,
    ) -> Result<StepOutcome, String> {
        self.resumes.fetch_add(1, Ordering::SeqCst);
        self.hold_a_slot().await
    }
}

/// A clean stream with the worker's group already in place, so messages added
/// before the worker starts are delivered to it (the worker's own `XGROUP CREATE`
/// only sees messages added after it runs).
async fn fresh_stream(redis: &redis::Client) -> redis::aio::MultiplexedConnection {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .expect("connect to redis — is `just infra` up?");
    let _: () = redis::cmd("DEL")
        .arg(STREAM_KEY)
        .query_async(&mut conn)
        .await
        .expect("clear the test stream");
    let _: () = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg("0")
        .arg("MKSTREAM")
        .query_async(&mut conn)
        .await
        .expect("create the consumer group");
    conn
}

async fn insert_user(db: &sqlx::PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(format!("maf-worker-{}", Uuid::new_v4().simple()))
        .bind(format!("maf-worker-{}@test.local", Uuid::new_v4().simple()))
        .fetch_one(db)
        .await
        .expect("insert user")
}

/// An execution row in `status`, with the empty step results a resume reads back.
async fn insert_execution(db: &sqlx::PgPool, user_id: Uuid, status: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO maf_executions (user_id, status, step_results) \
         VALUES ($1, $2, '[]'::jsonb) RETURNING id",
    )
    .bind(user_id)
    .bind(status)
    .fetch_one(db)
    .await
    .expect("insert execution")
}

/// Queues a fresh-run message for `execution_id`, or a resume when `resume` is set.
async fn queue(
    conn: &mut redis::aio::MultiplexedConnection,
    execution_id: Uuid,
    user_id: Uuid,
    resume: bool,
) {
    let mut xadd = redis::cmd("XADD");
    xadd.arg(STREAM_KEY)
        .arg("*")
        .arg("execution_id")
        .arg(execution_id.to_string())
        .arg("maf_json")
        .arg(r#"{"steps":[]}"#)
        .arg("user_id")
        .arg(user_id.to_string());
    if resume {
        xadd.arg("resume_step_index")
            .arg("0")
            .arg("resume_task_id")
            .arg("task-0")
            .arg("resume_answer")
            .arg("yes");
    }
    let _: String = xadd.query_async(conn).await.expect("queue message");
}

/// Queues `count` fresh runs of new `pending` executions.
async fn queue_executions(
    db: &sqlx::PgPool,
    conn: &mut redis::aio::MultiplexedConnection,
    count: usize,
) -> Vec<Uuid> {
    let user_id = insert_user(db).await;
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let id = insert_execution(db, user_id, "pending").await;
        queue(conn, id, user_id, false).await;
        ids.push(id);
    }
    ids
}

fn start(db: &sqlx::PgPool, redis: redis::Client, concurrency: usize) -> Arc<CountingExecutor> {
    let executor = Arc::new(CountingExecutor::default());
    let flow_guard = Arc::new(nasiko_flow::FlowGuard::new(
        redis.clone(),
        nasiko_flow::FlowConfig::default(),
    ));
    start_worker(
        db.clone(),
        redis,
        reqwest::Client::new(),
        flow_guard,
        WorkerConfig {
            llm: LlmConfig {
                api_key: "unused".into(),
                base_url: None,
                model: "unused".into(),
            },
            concurrency,
        },
        Arc::new(nasiko_hitl::PgHitlStore::new(db.clone())),
        executor.clone(),
    );
    executor
}

async fn status_of(db: &sqlx::PgPool, execution_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM maf_executions WHERE id = $1")
        .bind(execution_id)
        .fetch_one(db)
        .await
        .expect("read status")
}

/// Messages delivered to the group and not yet acknowledged.
async fn pending_count(conn: &mut redis::aio::MultiplexedConnection) -> i64 {
    let summary: redis::Value = redis::cmd("XPENDING")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .query_async(conn)
        .await
        .expect("XPENDING");
    match summary {
        redis::Value::Array(parts) => match parts.first() {
            Some(redis::Value::Int(n)) => *n,
            _ => 0,
        },
        _ => 0,
    }
}

/// Waits until every execution has finished successfully.
async fn wait_for_success(db: &sqlx::PgPool, ids: &[Uuid]) {
    for _ in 0..100 {
        let done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM maf_executions WHERE id = ANY($1) AND status = 'success'",
        )
        .bind(ids)
        .fetch_one(db)
        .await
        .expect("count finished executions");
        if done as usize == ids.len() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("not every execution finished within 10 s");
}

// ─── Concurrency ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn queued_executions_run_concurrently_up_to_the_limit() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let ids = queue_executions(&server.db, &mut conn, 5).await;

    let executor = start(&server.db, redis, 2);
    wait_for_success(&server.db, &ids).await;

    assert_eq!(
        executor.most_at_once.load(Ordering::SeqCst),
        2,
        "five queued runs with two slots: two at a time, never more"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_concurrency_of_one_runs_executions_one_at_a_time() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let ids = queue_executions(&server.db, &mut conn, 3).await;

    let executor = start(&server.db, redis, 1);
    wait_for_success(&server.db, &ids).await;

    assert_eq!(
        executor.most_at_once.load(Ordering::SeqCst),
        1,
        "one slot keeps the old serial behavior"
    );
    server.cleanup().await;
}

// ─── Which messages run ────────────────────────────────────────────────────

/// Queues a message for an execution in `status`, then a normal run behind it. With one
/// slot the worker takes them in order, so once the normal run has finished the first
/// message has been handled too.
async fn handle_then_drain(
    db: &sqlx::PgPool,
    redis: redis::Client,
    status: &str,
    resume: bool,
) -> (
    Uuid,
    Arc<CountingExecutor>,
    redis::aio::MultiplexedConnection,
) {
    let mut conn = fresh_stream(&redis).await;
    let user_id = insert_user(db).await;
    let execution_id = insert_execution(db, user_id, status).await;
    queue(&mut conn, execution_id, user_id, resume).await;
    let after = queue_executions(db, &mut conn, 1).await;

    let executor = start(db, redis, 1);
    wait_for_success(db, &after).await;
    (execution_id, executor, conn)
}

#[tokio::test]
#[serial]
async fn a_message_for_a_finished_execution_is_dropped_not_run_again() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");

    let (execution_id, executor, mut conn) =
        handle_then_drain(&server.db, redis, "failed", false).await;

    assert_eq!(
        executor.runs.load(Ordering::SeqCst),
        1,
        "only the run queued behind it ran"
    );
    assert_eq!(status_of(&server.db, execution_id).await, "failed");
    assert_eq!(
        pending_count(&mut conn).await,
        0,
        "the stale message is acknowledged"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_fresh_run_message_for_a_paused_execution_is_dropped() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");

    let (execution_id, executor, mut conn) =
        handle_then_drain(&server.db, redis, "awaiting_human", false).await;

    assert_eq!(executor.runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        status_of(&server.db, execution_id).await,
        "awaiting_human",
        "the pause is left for its own resume"
    );
    assert_eq!(pending_count(&mut conn).await, 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_resume_runs_while_the_row_still_says_awaiting_human() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");

    // The HITL dispatcher queues the resume before it moves the row to `pending`.
    let (execution_id, executor, _conn) =
        handle_then_drain(&server.db, redis, "awaiting_human", true).await;

    assert_eq!(executor.resumes.load(Ordering::SeqCst), 1);
    assert_eq!(status_of(&server.db, execution_id).await, "success");
    server.cleanup().await;
}
