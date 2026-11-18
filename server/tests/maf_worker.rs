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
use nasiko_orchestrator::maf::stop::{self, StopScope};
use nasiko_orchestrator::maf::types::{ExecutionResult, MafDefinition, StepOutcome};
use nasiko_orchestrator::maf::{
    AgentCalls, LlmConfig, MafExecutor, MafResume, MafRunContext, STREAM_KEY, WorkerConfig,
    start_worker,
};
use serial_test::serial;
use uuid::Uuid;

const GROUP_NAME: &str = "maf-workers";
/// How long each fake execution holds its slot, unless a test says otherwise.
const RUN_TIME: Duration = Duration::from_millis(300);
/// The heartbeat these tests run the worker with: recovery takes a message over after
/// six missed (6 s) and passes run every three (3 s).
const HEARTBEAT: Duration = Duration::from_secs(1);

fn test_redis_url() -> String {
    std::env::var("MAF_WORKER_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6379/15".into())
}

/// Counts runs and resumes, how many are inside the executor at once and the most there
/// ever were, and how many ran to the end or were stopped part-way.
struct CountingExecutor {
    run_time: Duration,
    runs: AtomicUsize,
    resumes: AtomicUsize,
    running: AtomicUsize,
    most_at_once: AtomicUsize,
    finished: AtomicUsize,
    stopped: AtomicUsize,
}

/// Counts a run dropped before it finished: the worker stopped it.
struct StopCounter<'a> {
    stopped: &'a AtomicUsize,
    finished: bool,
}

impl Drop for StopCounter<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl CountingExecutor {
    fn taking(run_time: Duration) -> Self {
        Self {
            run_time,
            runs: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            most_at_once: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            stopped: AtomicUsize::new(0),
        }
    }

    async fn hold_a_slot(&self) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_at_once.fetch_max(now, Ordering::SeqCst);
        let mut counter = StopCounter {
            stopped: &self.stopped,
            finished: false,
        };
        tokio::time::sleep(self.run_time).await;
        counter.finished = true;
        self.finished.fetch_add(1, Ordering::SeqCst);
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
    ) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.hold_a_slot().await
    }

    async fn resume(
        &self,
        _ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _resume: MafResume,
    ) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
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

/// The worker's agent calls, unused here: `CountingExecutor` never calls an agent.
fn agent_calls() -> AgentCalls {
    struct Unmetered;
    impl nasiko_types::agent_call::AgentCallMeter for Unmetered {
        fn begin(&self, _agent_id: Uuid) -> Box<dyn nasiko_types::agent_call::AgentCallHandle> {
            Box::new(nasiko_types::agent_call::UnmeteredCall)
        }
    }
    AgentCalls {
        client: reqwest::Client::new(),
        meter: Arc::new(Unmetered),
        outbound: Arc::new(nasiko_orchestrator::outbound::NoOutboundPrefix),
        clock: nasiko_types::agent_clock::AgentClock::OFF,
        // Never asked: `CountingExecutor` never calls an agent, so the pool is never dialled.
        targets: Arc::new(nasiko_orchestrator::DirectAddress::new(
            sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
                .expect("connect_lazy never actually connects"),
        )),
    }
}

fn start(db: &sqlx::PgPool, redis: redis::Client, concurrency: usize) -> Arc<CountingExecutor> {
    start_with(db, redis, concurrency, CountingExecutor::taking(RUN_TIME))
}

fn start_with<E: MafExecutor + 'static>(
    db: &sqlx::PgPool,
    redis: redis::Client,
    concurrency: usize,
    executor: E,
) -> Arc<E> {
    let executor = Arc::new(executor);
    let flow_guard = Arc::new(nasiko_flow::FlowGuard::new(
        nasiko_redis::SharedRedis::new(redis.clone()),
        nasiko_flow::FlowConfig::default(),
    ));
    start_worker(
        db.clone(),
        redis,
        reqwest::Client::new(),
        agent_calls(),
        flow_guard,
        WorkerConfig {
            llm: LlmConfig {
                api_key: "unused".into(),
                base_url: None,
                model: "unused".into(),
            },
            ledger: Arc::new(nasiko_pricing::NoUsageLedger),
            backend: None,
            concurrency,
            heartbeat: HEARTBEAT,
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

/// Entries left in the stream.
async fn stream_len(conn: &mut redis::aio::MultiplexedConnection) -> i64 {
    redis::cmd("XLEN")
        .arg(STREAM_KEY)
        .query_async(conn)
        .await
        .expect("XLEN")
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

/// The consumer `msg_id` is pending to, and its delivery count.
async fn pending_entry(
    conn: &mut redis::aio::MultiplexedConnection,
    msg_id: &str,
) -> Option<(String, i64)> {
    let reply: redis::Value = redis::cmd("XPENDING")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg(msg_id)
        .arg(msg_id)
        .arg(1)
        .query_async(conn)
        .await
        .expect("XPENDING");
    let redis::Value::Array(entries) = reply else {
        return None;
    };
    let redis::Value::Array(entry) = entries.into_iter().next()? else {
        return None;
    };
    match (entry.get(1)?, entry.get(3)?) {
        (redis::Value::BulkString(consumer), redis::Value::Int(count)) => {
            Some((String::from_utf8_lossy(consumer).into_owned(), *count))
        }
        _ => None,
    }
}

/// Delivers the stream's next message to `consumer`, a worker that then stops: the message
/// stays pending to it, untouched for `idle_ms` and already delivered `deliveries` times.
async fn leave_pending_to(
    conn: &mut redis::aio::MultiplexedConnection,
    consumer: &str,
    idle_ms: u64,
    deliveries: i64,
) -> String {
    let read: redis::Value = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg(GROUP_NAME)
        .arg(consumer)
        .arg("COUNT")
        .arg(1)
        .arg("STREAMS")
        .arg(STREAM_KEY)
        .arg(">")
        .query_async(conn)
        .await
        .expect("XREADGROUP");
    let msg_id = first_message_id(&read).expect("a message to leave pending");
    let _: redis::Value = redis::cmd("XCLAIM")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg(consumer)
        .arg(0)
        .arg(&msg_id)
        .arg("IDLE")
        .arg(idle_ms)
        .arg("RETRYCOUNT")
        .arg(deliveries)
        .arg("JUSTID")
        .query_async(conn)
        .await
        .expect("XCLAIM");
    msg_id
}

/// The id of the first message in an XREADGROUP reply.
fn first_message_id(reply: &redis::Value) -> Option<String> {
    let redis::Value::Array(streams) = reply else {
        return None;
    };
    let redis::Value::Array(stream) = streams.first()? else {
        return None;
    };
    let redis::Value::Array(messages) = stream.get(1)? else {
        return None;
    };
    let redis::Value::Array(message) = messages.first()? else {
        return None;
    };
    match message.first()? {
        redis::Value::BulkString(id) => Some(String::from_utf8_lossy(id).into_owned()),
        _ => None,
    }
}

/// Waits up to `within` for `done` to hold.
async fn wait_until(within: Duration, mut done: impl AsyncFnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if done().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
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

// ─── Heartbeat and recovery ────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn a_run_longer_than_the_takeover_threshold_keeps_its_message_and_runs_once() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let ids = queue_executions(&server.db, &mut conn, 1).await;

    // 13 s of work against a 6 s takeover threshold, with this worker's own recovery
    // passing every 3 s (checked between 2 s reads): without the heartbeat a pass would
    // find the message idle past the threshold and start the run a second time.
    let executor = start_with(
        &server.db,
        redis,
        2,
        CountingExecutor::taking(Duration::from_secs(13)),
    );
    let finished = wait_until(Duration::from_secs(20), async || {
        status_of(&server.db, ids[0]).await == "success"
    })
    .await;

    assert!(finished, "the long run finished");
    assert_eq!(
        executor.runs.load(Ordering::SeqCst),
        1,
        "it ran exactly once"
    );
    assert_eq!(executor.stopped.load(Ordering::SeqCst), 0);
    assert_eq!(pending_count(&mut conn).await, 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_run_whose_message_is_taken_over_stops_without_recording_its_outcome() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let ids = queue_executions(&server.db, &mut conn, 1).await;
    let executor = start_with(
        &server.db,
        redis,
        1,
        CountingExecutor::taking(Duration::from_secs(5)),
    );
    let started = wait_until(Duration::from_secs(5), async || {
        executor.runs.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(started, "the run started");

    // Another worker's recovery takes the message over while the run is still working.
    let msg_id = {
        let read: redis::Value = redis::cmd("XRANGE")
            .arg(STREAM_KEY)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .expect("XRANGE");
        let redis::Value::Array(messages) = read else {
            panic!("no messages")
        };
        let redis::Value::Array(message) = &messages[0] else {
            panic!("bad message")
        };
        let redis::Value::BulkString(id) = &message[0] else {
            panic!("bad id")
        };
        String::from_utf8_lossy(id).into_owned()
    };
    let _: redis::Value = redis::cmd("XCLAIM")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg("maf-worker-intruder")
        .arg(0)
        .arg(&msg_id)
        .arg("JUSTID")
        .query_async(&mut conn)
        .await
        .expect("XCLAIM");

    let stopped = wait_until(Duration::from_secs(3), async || {
        executor.stopped.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(
        stopped,
        "the next heartbeat saw the takeover and stopped the run"
    );
    assert_eq!(executor.finished.load(Ordering::SeqCst), 0);
    assert_eq!(
        status_of(&server.db, ids[0]).await,
        "running",
        "no outcome was recorded over the new owner's run"
    );
    assert_eq!(
        pending_entry(&mut conn, &msg_id)
            .await
            .map(|(owner, _)| owner)
            .as_deref(),
        Some("maf-worker-intruder"),
        "the message was left for its new owner, not acknowledged"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_message_left_by_a_stopped_worker_is_recovered_without_a_restart() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let ids = queue_executions(&server.db, &mut conn, 1).await;
    // Untouched for 4 s when the worker starts: under the 6 s threshold, so its startup
    // pass leaves it, and only a later periodic pass can take it over.
    leave_pending_to(&mut conn, "maf-worker-stopped", 4_000, 1).await;

    let executor = start(&server.db, redis, 1);
    let recovered = wait_until(Duration::from_secs(12), async || {
        status_of(&server.db, ids[0]).await == "success"
    })
    .await;

    assert!(
        recovered,
        "a periodic recovery pass took the message over and ran it"
    );
    assert_eq!(executor.runs.load(Ordering::SeqCst), 1);
    assert_eq!(pending_count(&mut conn).await, 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_message_whose_worker_kept_stopping_fails_its_execution_instead_of_running() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let ids = queue_executions(&server.db, &mut conn, 1).await;
    // Delivered three times already, and abandoned again: the startup pass takes it over
    // as its fourth delivery.
    leave_pending_to(&mut conn, "maf-worker-stopped", 60_000, 3).await;

    let executor = start(&server.db, redis, 1);
    let given_up = wait_until(Duration::from_secs(5), async || {
        status_of(&server.db, ids[0]).await == "failed"
    })
    .await;

    assert!(given_up, "the execution was failed");
    assert_eq!(
        executor.runs.load(Ordering::SeqCst),
        0,
        "it was not run again"
    );
    let error: Option<String> =
        sqlx::query_scalar("SELECT error FROM maf_executions WHERE id = $1")
            .bind(ids[0])
            .fetch_one(&server.db)
            .await
            .expect("read error");
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("stopped 3 times")),
        "the failure says why: {error:?}"
    );
    assert_eq!(
        pending_count(&mut conn).await,
        0,
        "the message is acknowledged"
    );
    server.cleanup().await;
}

// ─── Stopping ──────────────────────────────────────────────────────────────

/// A workflow of `user_id`'s: a stop is its owner's to make.
async fn insert_workflow(db: &sqlx::PgPool, user_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO mafs (user_id, name, maf_json) VALUES ($1, $2, '{\"steps\":[]}'::jsonb) RETURNING id",
    )
    .bind(user_id)
    .bind(format!("maf-stop-{}", Uuid::new_v4().simple()))
    .fetch_one(db)
    .await
    .expect("insert workflow")
}

/// A run of `maf_id` in `status`.
async fn insert_run(db: &sqlx::PgPool, user_id: Uuid, maf_id: Uuid, status: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO maf_executions (user_id, maf_id, status, step_results) \
         VALUES ($1, $2, $3, '[]'::jsonb) RETURNING id",
    )
    .bind(user_id)
    .bind(maf_id)
    .bind(status)
    .fetch_one(db)
    .await
    .expect("insert run")
}

/// A queued run of a new workflow of a new user: `(user, run)`.
async fn queue_owned_run(
    db: &sqlx::PgPool,
    conn: &mut redis::aio::MultiplexedConnection,
) -> (Uuid, Uuid) {
    let user_id = insert_user(db).await;
    let maf_id = insert_workflow(db, user_id).await;
    let run = insert_run(db, user_id, maf_id, "pending").await;
    queue(conn, run, user_id, false).await;
    (user_id, run)
}

/// Runs `steps` steps of `step_time` each, writing its progress after each one as the real
/// executors do; counts the steps done and whether it was cut short.
struct SteppingExecutor {
    steps: usize,
    step_time: Duration,
    done: AtomicUsize,
    cut_short: AtomicUsize,
}

#[async_trait]
impl MafExecutor for SteppingExecutor {
    async fn run(
        &self,
        ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _content: Option<&str>,
    ) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
        let mut counter = StopCounter {
            stopped: &self.cut_short,
            finished: false,
        };
        for _ in 0..self.steps {
            tokio::time::sleep(self.step_time).await;
            nasiko_orchestrator::maf::executor::persist_progress(
                ctx.db,
                ctx.execution_id,
                &[],
                0,
                0.0,
            )
            .await;
            self.done.fetch_add(1, Ordering::SeqCst);
        }
        counter.finished = true;
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
    ) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
        Err("not resumable".into())
    }
}

#[tokio::test]
#[serial]
async fn a_stopped_queued_run_is_discarded_without_running() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let (user_id, run) = queue_owned_run(&server.db, &mut conn).await;

    let report = stop::stop_in_db(&server.db, user_id, StopScope::Executions(&[run]))
        .await
        .expect("stop");
    assert_eq!(report.stopped, [(run, "pending".to_string())]);
    assert_eq!(
        stop::notify_owners(&mut conn, &[run])
            .await
            .expect("lookup"),
        0,
        "no worker holds a run that is still queued"
    );

    let after = queue_executions(&server.db, &mut conn, 1).await;
    let executor = start(&server.db, redis, 1);
    wait_for_success(&server.db, &after).await;

    assert_eq!(
        executor.runs.load(Ordering::SeqCst),
        1,
        "only the run queued behind it ran"
    );
    assert_eq!(status_of(&server.db, run).await, "stopped");
    assert_eq!(
        pending_count(&mut conn).await,
        0,
        "its message was acknowledged"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn stopping_a_running_execution_cuts_it_short_on_its_worker() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let (user_id, run) = queue_owned_run(&server.db, &mut conn).await;
    let executor = start_with(
        &server.db,
        redis.clone(),
        1,
        CountingExecutor::taking(Duration::from_secs(30)),
    );
    let started = wait_until(Duration::from_secs(5), async || {
        executor.runs.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(started, "the run started");

    let report = stop::stop_in_db(&server.db, user_id, StopScope::Executions(&[run]))
        .await
        .expect("stop");
    assert_eq!(report.were_running(), [run]);
    let workers = stop::notify_owners(&mut conn, &report.were_running())
        .await
        .expect("notify");
    assert_eq!(workers, 1, "the one worker holding the run was found");

    let cut_short = wait_until(Duration::from_secs(3), async || {
        executor.stopped.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(cut_short, "the run was dropped mid-way, not left to finish");
    assert_eq!(executor.finished.load(Ordering::SeqCst), 0);
    assert_eq!(status_of(&server.db, run).await, "stopped");
    let acknowledged = wait_until(Duration::from_secs(3), async || {
        pending_count(&mut conn).await == 0
    })
    .await;
    assert!(
        acknowledged,
        "the worker acknowledged the stopped run's message"
    );
    assert_eq!(
        stream_len(&mut conn).await,
        0,
        "and deleted it from the stream"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_run_whose_stop_message_is_lost_ends_at_its_next_progress_write() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let (user_id, run) = queue_owned_run(&server.db, &mut conn).await;
    let executor = start_with(
        &server.db,
        redis,
        1,
        SteppingExecutor {
            steps: 50,
            step_time: Duration::from_millis(200),
            done: AtomicUsize::new(0),
            cut_short: AtomicUsize::new(0),
        },
    );
    let stepping = wait_until(Duration::from_secs(5), async || {
        executor.done.load(Ordering::SeqCst) >= 1
    })
    .await;
    assert!(stepping, "the run is taking steps");

    // Stopped in the database only: no message reaches the worker.
    stop::stop_in_db(&server.db, user_id, StopScope::Executions(&[run]))
        .await
        .expect("stop");

    let ended = wait_until(Duration::from_secs(3), async || {
        executor.cut_short.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(
        ended,
        "the next progress write found the run stopped and ended it"
    );
    assert!(
        executor.done.load(Ordering::SeqCst) < 50,
        "it did not run to completion"
    );
    assert_eq!(status_of(&server.db, run).await, "stopped");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_run_that_finishes_after_its_stop_stays_stopped() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let (user_id, run) = queue_owned_run(&server.db, &mut conn).await;
    // Writes no progress and gets no message, so nothing cuts it short: it finishes.
    let executor = start_with(
        &server.db,
        redis,
        1,
        CountingExecutor::taking(Duration::from_secs(1)),
    );
    let started = wait_until(Duration::from_secs(5), async || {
        executor.runs.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(started, "the run started");
    stop::stop_in_db(&server.db, user_id, StopScope::Executions(&[run]))
        .await
        .expect("stop");

    let finished = wait_until(Duration::from_secs(3), async || {
        executor.finished.load(Ordering::SeqCst) == 1 && pending_count(&mut conn).await == 0
    })
    .await;
    assert!(
        finished,
        "the run finished and its message was acknowledged"
    );
    assert_eq!(
        status_of(&server.db, run).await,
        "stopped",
        "its success was not written over the stop"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn stopping_a_paused_run_withdraws_its_question_and_its_waiting_step() {
    let server = common::TestServer::start().await;
    let user_id = insert_user(&server.db).await;
    let maf_id = insert_workflow(&server.db, user_id).await;
    let run = insert_run(&server.db, user_id, maf_id, "awaiting_human").await;
    sqlx::query(
        "UPDATE maf_executions SET step_results = \
         '[{\"step_index\":0,\"status\":\"success\"},{\"step_index\":1,\"status\":\"awaiting_human\"},{\"step_index\":2,\"status\":\"pending\"}]'::jsonb \
         WHERE id = $1",
    )
    .bind(run)
    .execute(&server.db)
    .await
    .expect("set steps");
    let agent: Uuid =
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("maf-stop-agent-{}", Uuid::new_v4().simple()))
            .bind(user_id)
            .fetch_one(&server.db)
            .await
            .expect("insert agent");
    let question: Uuid = sqlx::query_scalar(
        "INSERT INTO hitl_requests (kind, origin, agent_id, owner_user_id, task_id, context_id, \
                                    maf_execution_id, maf_step_index, question) \
         VALUES ('input_required', 'maf', $1, $2, 'task-1', $3, $3, 1, '{\"message\":\"which one?\"}') \
         RETURNING id",
    )
    .bind(agent)
    .bind(user_id)
    .bind(run)
    .fetch_one(&server.db)
    .await
    .expect("insert hitl request");

    // What the run spent before it paused: MAF's own calls, and an agent's call under the same
    // trace that is not MAF's spend. The row itself says 0, as a paused run's row does.
    for (agent_name, input, output, cached, cost) in [
        ("maf", 100, 20, 0, 0.01),
        ("maf:step0", 50, 5, 5, 0.002),
        ("some-agent", 1000, 1000, 0, 1.0),
    ] {
        sqlx::query(
            "INSERT INTO trace_usage (trace_id, agent_name, started_at, input_tokens, output_tokens, \
                                      cache_read_tokens, cost_usd) \
             VALUES ($1, $2, now(), $3, $4, $5, $6)",
        )
        .bind(run.to_string())
        .bind(agent_name)
        .bind(input as i64)
        .bind(output as i64)
        .bind(cached as i64)
        .bind(cost)
        .execute(&server.db)
        .await
        .expect("insert usage");
    }

    let report = stop::stop_in_db(&server.db, user_id, StopScope::Executions(&[run]))
        .await
        .expect("stop");

    assert_eq!(report.stopped, [(run, "awaiting_human".to_string())]);
    let (tokens, cost): (i64, f64) =
        sqlx::query_as("SELECT tokens_used, cost_usd FROM maf_executions WHERE id = $1")
            .bind(run)
            .fetch_one(&server.db)
            .await
            .expect("read spend");
    assert_eq!(
        tokens, 180,
        "MAF's own calls, as the platform recorded them"
    );
    assert!((cost - 0.012).abs() < 1e-9, "MAF's own cost, got {cost}");
    assert!(report.were_running().is_empty(), "no worker to tell");
    assert_eq!(status_of(&server.db, run).await, "stopped");
    let question_status: String =
        sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
            .bind(question)
            .fetch_one(&server.db)
            .await
            .expect("read hitl status");
    assert_eq!(question_status, "canceled");
    let steps: Vec<String> = sqlx::query_scalar(
        "SELECT s->>'status' FROM maf_executions, jsonb_array_elements(step_results) s WHERE id = $1",
    )
    .bind(run)
    .fetch_all(&server.db)
    .await
    .expect("read steps");
    assert_eq!(
        steps,
        ["success", "stopped", "stopped"],
        "finished steps keep their outcome; the rest are stopped"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn only_the_callers_unfinished_runs_are_stopped() {
    let server = common::TestServer::start().await;
    let owner = insert_user(&server.db).await;
    let workflow = insert_workflow(&server.db, owner).await;
    let queued = insert_run(&server.db, owner, workflow, "pending").await;
    let done = insert_run(&server.db, owner, workflow, "success").await;
    let someone = insert_user(&server.db).await;
    let theirs_workflow = insert_workflow(&server.db, someone).await;
    let theirs = insert_run(&server.db, someone, theirs_workflow, "pending").await;
    let unknown = Uuid::new_v4();
    // A run whose workflow row is gone (`maf_id` NULL): still the owner's run.
    let orphan = insert_execution(&server.db, owner, "pending").await;

    let report = stop::stop_in_db(
        &server.db,
        owner,
        StopScope::Executions(&[queued, done, theirs, unknown, orphan]),
    )
    .await
    .expect("stop");

    let mut stopped = report.stopped.clone();
    stopped.sort();
    let mut expected = vec![
        (queued, "pending".to_string()),
        (orphan, "pending".to_string()),
    ];
    expected.sort();
    assert_eq!(stopped, expected);
    assert_eq!(report.already_finished, [(done, "success".to_string())]);
    let mut not_found = report.not_found.clone();
    not_found.sort();
    let mut expected = vec![theirs, unknown];
    expected.sort();
    assert_eq!(not_found, expected, "another user's run reads as not found");
    assert_eq!(status_of(&server.db, theirs).await, "pending");
    assert_eq!(status_of(&server.db, done).await, "success");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_workflow_stop_stops_every_unfinished_run_of_that_workflow_only() {
    let server = common::TestServer::start().await;
    let owner = insert_user(&server.db).await;
    let workflow = insert_workflow(&server.db, owner).await;
    let queued = insert_run(&server.db, owner, workflow, "pending").await;
    let paused = insert_run(&server.db, owner, workflow, "awaiting_human").await;
    let done = insert_run(&server.db, owner, workflow, "failed").await;
    let other_workflow = insert_workflow(&server.db, owner).await;
    let elsewhere = insert_run(&server.db, owner, other_workflow, "pending").await;

    let report = stop::stop_in_db(&server.db, owner, StopScope::Workflow(workflow))
        .await
        .expect("stop");

    let mut stopped: Vec<Uuid> = report.stopped.iter().map(|(id, _)| *id).collect();
    stopped.sort();
    let mut expected = vec![queued, paused];
    expected.sort();
    assert_eq!(stopped, expected);
    assert_eq!(status_of(&server.db, done).await, "failed");
    assert_eq!(status_of(&server.db, elsewhere).await, "pending");
    server.cleanup().await;
}

// ─── Stream cleanup ────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn acknowledged_messages_are_deleted_from_the_stream() {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");

    // A stale message (discarded) and a run (finished) behind it.
    let (_execution_id, executor, mut conn) =
        handle_then_drain(&server.db, redis, "failed", false).await;
    assert_eq!(executor.runs.load(Ordering::SeqCst), 1);

    let emptied = wait_until(Duration::from_secs(3), async || {
        stream_len(&mut conn).await == 0
    })
    .await;
    assert!(
        emptied,
        "neither the finished run's entry nor the discarded one is left in the stream"
    );
    assert_eq!(pending_count(&mut conn).await, 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn overlapping_stops_of_the_same_runs_never_deadlock() {
    let server = common::TestServer::start().await;
    let owner = insert_user(&server.db).await;
    let workflow = insert_workflow(&server.db, owner).await;
    for round in 0..5 {
        let mut runs = Vec::new();
        for _ in 0..40 {
            runs.push(insert_run(&server.db, owner, workflow, "pending").await);
        }
        let mut reversed = runs.clone();
        reversed.reverse();
        // The same runs, asked for in opposite orders and by workflow, all at once.
        let (by_ids, by_reversed_ids, by_workflow) = tokio::join!(
            stop::stop_in_db(&server.db, owner, StopScope::Executions(&runs)),
            stop::stop_in_db(&server.db, owner, StopScope::Executions(&reversed)),
            stop::stop_in_db(&server.db, owner, StopScope::Workflow(workflow)),
        );
        let stopped = by_ids.expect("stop by ids").stopped.len()
            + by_reversed_ids.expect("stop by reversed ids").stopped.len()
            + by_workflow.expect("stop by workflow").stopped.len();
        assert_eq!(
            stopped,
            runs.len(),
            "round {round}: each run stopped exactly once"
        );
    }
    server.cleanup().await;
}

/// Ends every run it is given with `error`, counting how often it was asked.
struct FailingExecutor {
    error: nasiko_orchestrator::maf::types::RunError,
    runs: AtomicUsize,
}

#[async_trait]
impl MafExecutor for FailingExecutor {
    async fn run(
        &self,
        _ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _content: Option<&str>,
    ) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Err(self.error.clone())
    }

    async fn resume(
        &self,
        _ctx: &MafRunContext<'_>,
        _maf_def: &MafDefinition,
        _resume: MafResume,
    ) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
        Err(self.error.clone())
    }
}

/// Runs one execution through a worker whose executor always ends it with `error`, and returns
/// how many times it ran, its final status, its error and its attempt count.
async fn run_to_failure(
    error: nasiko_orchestrator::maf::types::RunError,
) -> (usize, String, Option<String>, i32) {
    let server = common::TestServer::start().await;
    let redis = redis::Client::open(test_redis_url()).expect("redis url");
    let mut conn = fresh_stream(&redis).await;
    let ids = queue_executions(&server.db, &mut conn, 1).await;

    let executor = start_with(
        &server.db,
        redis,
        1,
        FailingExecutor {
            error,
            runs: AtomicUsize::new(0),
        },
    );
    assert!(
        wait_until(Duration::from_secs(20), async || {
            status_of(&server.db, ids[0]).await == "failed"
        })
        .await,
        "the execution ends failed"
    );
    // A retry, were one coming, would already be queued: give it the time to show.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (status, error, attempts): (String, Option<String>, i32) =
        sqlx::query_as("SELECT status, error, attempt_count FROM maf_executions WHERE id = $1")
            .bind(ids[0])
            .fetch_one(&server.db)
            .await
            .expect("read execution");
    let runs = executor.runs.load(Ordering::SeqCst);
    server.cleanup().await;
    (runs, status, error, attempts)
}

/// An agent that timed out would only time out again, after the steps before it ran a second
/// time: a timed-out run ends on its first attempt, with its reason.
#[tokio::test]
#[serial]
async fn a_timed_out_run_fails_without_a_retry() {
    let reason = "step 1 (agent 'task-breakdown') timed out after 600s";
    let (runs, status, error, attempts) = run_to_failure(
        nasiko_orchestrator::maf::types::RunError::TimedOut(reason.into()),
    )
    .await;

    assert_eq!(runs, 1, "run once, not retried");
    assert_eq!(attempts, 1);
    assert_eq!(status, "failed");
    assert_eq!(error.as_deref(), Some(reason));
}

#[tokio::test]
#[serial]
async fn any_other_failure_is_still_retried_up_to_its_attempt_limit() {
    let (runs, status, _, attempts) = run_to_failure(
        nasiko_orchestrator::maf::types::RunError::Failed("HTTP 502 Bad Gateway".into()),
    )
    .await;

    assert_eq!(runs, 3, "the default MAF_MAX_ATTEMPTS");
    assert_eq!(attempts, 3);
    assert_eq!(status, "failed");
}
