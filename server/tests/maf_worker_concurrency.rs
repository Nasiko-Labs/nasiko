/// Integration tests for the MAF worker's concurrency: queued executions run at
/// the same time, never more of them than `WorkerConfig::concurrency`.
///
/// Requires: `just infra`
/// Run with:  `cargo test -p nasiko-server --test maf_worker_concurrency -- --test-threads=1`
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

/// Counts how many executions are inside `run` at once, and the most there ever were.
#[derive(Default)]
struct CountingExecutor {
    running: AtomicUsize,
    most_at_once: AtomicUsize,
}

#[async_trait]
impl MafExecutor for CountingExecutor {
    async fn run(
        &self,
        _ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _content: Option<&str>,
    ) -> Result<StepOutcome, String> {
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

    async fn resume(
        &self,
        _ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _resume: MafResume,
    ) -> Result<StepOutcome, String> {
        Err("these tests queue no resumes".into())
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

/// Queues `count` executions, each with a `maf_executions` row the worker can update.
async fn queue_executions(
    db: &sqlx::PgPool,
    conn: &mut redis::aio::MultiplexedConnection,
    count: usize,
) -> Vec<Uuid> {
    let user_id: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("maf-worker-{}", Uuid::new_v4().simple()))
            .bind(format!("maf-worker-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(db)
            .await
            .expect("insert user");

    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO maf_executions (user_id, status) VALUES ($1, 'pending') RETURNING id",
        )
        .bind(user_id)
        .fetch_one(db)
        .await
        .expect("insert execution");
        let _: String = redis::cmd("XADD")
            .arg(STREAM_KEY)
            .arg("*")
            .arg("execution_id")
            .arg(id.to_string())
            .arg("maf_json")
            .arg(r#"{"steps":[]}"#)
            .arg("user_id")
            .arg(user_id.to_string())
            .query_async(conn)
            .await
            .expect("queue execution");
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
