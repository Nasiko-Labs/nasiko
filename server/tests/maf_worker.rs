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

    async fn hold_a_slot(&self) -> Result<StepOutcome, String> {
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
    }
}

fn start(db: &sqlx::PgPool, redis: redis::Client, concurrency: usize) -> Arc<CountingExecutor> {
    start_with(db, redis, concurrency, CountingExecutor::taking(RUN_TIME))
}

fn start_with(
    db: &sqlx::PgPool,
    redis: redis::Client,
    concurrency: usize,
    executor: CountingExecutor,
) -> Arc<CountingExecutor> {
    let executor = Arc::new(executor);
    let flow_guard = Arc::new(nasiko_flow::FlowGuard::new(
        redis.clone(),
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
