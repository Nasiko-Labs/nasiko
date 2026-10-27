use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use futures::FutureExt;
use nasiko_flow::FlowGuard;
use nasiko_hitl::{HitlStore, NewHitlRequest};
use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::{
    executor,
    llm::LlmClient,
    runner::{MafExecutor, MafResume, MafRunContext},
    types::{MafDefinition, PausedStep, StepOutcome, StepResult},
};

/// Redis stream this worker consumes from. `pub` (not just crate-visible) so every producer —
/// `oss/server/src/maf.rs` (the initial run) and `oss/server/src/hitl/mod.rs` (a resume re-enqueue)
/// — binds to this same constant rather than hardcoding the literal a second and third time; a
/// drift between the two used to mean silently orphaned jobs, no compile error and no runtime
/// error (found in review).
pub const STREAM_KEY: &str = "nasiko:maf:execute";
const GROUP_NAME: &str = "maf-workers";

/// Recovery takes a message over once the worker running it has missed this many
/// heartbeats in a row: a live run refreshes its message every heartbeat, so a message
/// idle this long belongs to a worker that stopped.
const MISSED_HEARTBEATS: u32 = 6;
/// A recovery pass starts this many heartbeats after the previous one ended.
const HEARTBEATS_BETWEEN_PASSES: u32 = 3;
/// At most this many XAUTOCLAIM calls per recovery pass, so a long pending list can't
/// keep the loop from reading new work. A pass that runs out stops where it is, and the
/// next pass continues from there.
const CLAIM_CALLS_PER_PASS: u32 = 10;
/// Deliveries after which recovery gives a message up: one first read plus two
/// recoveries. A message delivered more often was being run by a worker that stopped
/// each time (out of memory, killed), so running it again would stop the next worker
/// too; its execution is failed instead.
const MAX_DELIVERIES: i64 = 3;

/// Atomically: if message `ARGV[2]` is still pending to consumer `ARGV[3]`, reset its
/// idle time and return 1; otherwise (another worker took it over, or it is gone)
/// return 0. `JUSTID` keeps the refresh out of the message's delivery count, so only
/// real deliveries count towards `MAX_DELIVERIES`.
static REFRESH_OWNERSHIP: LazyLock<redis::Script> = LazyLock::new(|| {
    redis::Script::new(
        r"
        local pending = redis.call('XPENDING', KEYS[1], ARGV[1], ARGV[2], ARGV[2], 1)
        if pending[1] and pending[1][2] == ARGV[3] then
            redis.call('XCLAIM', KEYS[1], ARGV[1], ARGV[3], 0, ARGV[2], 'JUSTID')
            return 1
        end
        return 0
        ",
    )
});

/// How this worker keeps hold of the messages it is running, shared by every job.
struct Lease {
    consumer: String,
    heartbeat: Duration,
}

impl Lease {
    /// How long a message must sit untouched before recovery takes it over.
    fn abandoned_after(&self) -> Duration {
        self.heartbeat * MISSED_HEARTBEATS
    }

    /// How long after one recovery pass ends the next one starts.
    fn pass_every(&self) -> Duration {
        self.heartbeat * HEARTBEATS_BETWEEN_PASSES
    }
}

/// The recovery of messages whose worker stopped: a pass at startup, then one every
/// `Lease::pass_every`, each at most `CLAIM_CALLS_PER_PASS` XAUTOCLAIM calls long.
struct Recovery {
    /// Where the running pass continues from; `None` between passes.
    cursor: Option<String>,
    /// Where the next pass starts: the beginning, or where a pass that ran out of
    /// calls stopped.
    next_start: String,
    calls_left: u32,
    last_pass_ended: Instant,
}

impl Recovery {
    /// The startup pass is already running.
    fn at_startup() -> Self {
        Self {
            cursor: Some("0-0".to_string()),
            next_start: "0-0".to_string(),
            calls_left: CLAIM_CALLS_PER_PASS,
            last_pass_ended: Instant::now(),
        }
    }

    /// The cursor to claim from now: the running pass's, or a new pass's once one is due.
    /// `None` when no pass is running or due.
    fn take_call(&mut self, pass_every: Duration) -> Option<String> {
        if self.cursor.is_none() && self.last_pass_ended.elapsed() >= pass_every {
            self.cursor = Some(std::mem::replace(&mut self.next_start, "0-0".to_string()));
            self.calls_left = CLAIM_CALLS_PER_PASS;
        }
        let cursor = self.cursor.take()?;
        self.calls_left = self.calls_left.saturating_sub(1);
        Some(cursor)
    }

    /// Records where a claim left off: `None` at the end of the pending list, or the
    /// cursor to continue from — in this pass while it has calls left, else the next.
    fn finish_call(&mut self, next: Option<String>) {
        match next {
            None => self.end_pass("0-0".to_string()),
            Some(cursor) if self.calls_left == 0 => self.end_pass(cursor),
            Some(cursor) => self.cursor = Some(cursor),
        }
    }

    fn end_pass(&mut self, next_start: String) {
        self.next_start = next_start;
        self.last_pass_ended = Instant::now();
    }
}

/// Unique per process: pod name (HOSTNAME in k8s/docker) + OS PID.
/// Two pods or two local processes will never share the same name, so Redis
/// can track their pending-entry lists independently.
fn consumer_name() -> String {
    let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "worker".into());
    format!("maf-worker-{hostname}-{}", std::process::id())
}

/// What every job needs, shared by all the jobs a worker runs at once. Cloning it copies
/// handles (the pool, the clients, `Arc`s), never the resources behind them.
#[derive(Clone)]
pub(super) struct JobDeps {
    pub(super) db: PgPool,
    /// How each job reaches its steps' agents: the client, the in-flight meter and the
    /// outbound prefix (`super::AgentCalls`).
    pub(super) agent_calls: super::AgentCalls,
    pub(super) flow_guard: Arc<FlowGuard>,
    pub(super) llm: LlmClient,
    pub(super) hitl_store: Arc<dyn HitlStore>,
    pub(super) maf_executor: Arc<dyn MafExecutor>,
}

/// Main MAF worker loop. Spawned once at server startup.
///
/// Keeps up to `concurrency` executions in flight, the same claim/spawn shape as
/// `build_worker::run` (`oss/server/src/agents/build_worker.rs`): the loop only takes
/// messages from the stream, and each execution runs in its own task tracked by a
/// `JoinSet`. An execution spends nearly all its time waiting on agents and LLMs, so
/// awaiting each one before reading the next — as this loop used to — left every
/// other queued run, resumes included, waiting behind the slowest one.
///
/// A message is only read when a slot is free to start it, so a busy worker leaves
/// the rest of the stream to workers that can start it now.
///
/// A run stays pending in the stream until it finishes, and refreshes its message every
/// `heartbeat` while it works (`hold_message`). Recovery passes (`Recovery`) take over
/// only messages that have gone `MISSED_HEARTBEATS` heartbeats without one, so a run is
/// never taken from a live worker however long it takes, and one whose worker stopped
/// is picked up by any live worker within minutes, not at the next restart.
pub(super) async fn run(
    deps: JobDeps,
    redis: redis::Client,
    concurrency: usize,
    heartbeat: Duration,
) {
    // `Config` clamps both, but a 0 concurrency would make the drain unreachable and park
    // the worker forever (the same guard as `build_worker::run`), and a zero heartbeat
    // would make every interval built from it panic.
    let concurrency = concurrency.max(1);
    let lease = Arc::new(Lease {
        consumer: consumer_name(),
        heartbeat: heartbeat.max(Duration::from_secs(1)),
    });
    let consumer = lease.consumer.as_str();

    // Two connections, never one: Redis serves a connection's commands in order, so
    // while the reader's XREADGROUP blocks (up to 2 s) nothing else sent on that
    // connection is processed. Jobs send their XACK / XADD on their own connection
    // so they never queue behind the read.
    let connections = (
        redis.get_multiplexed_async_connection().await,
        redis.get_multiplexed_async_connection().await,
    );
    let (mut reader, jobs_conn) = match connections {
        (Ok(reader), Ok(jobs_conn)) => (reader, jobs_conn),
        (Err(e), _) | (_, Err(e)) => {
            error!("MAF worker: failed to connect to Redis: {e}");
            return;
        }
    };

    // Create consumer group if it doesn't exist ('$' = only new messages; MKSTREAM creates stream)
    let _: redis::RedisResult<()> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg("$")
        .arg("MKSTREAM")
        .query_async(&mut reader)
        .await;

    // Messages whose worker stopped are reclaimed in batches the free slots can take,
    // through the same drain as new work, instead of all of them running one after
    // another before any new job is read.
    let mut recovery = Recovery::at_startup();
    let mut jobs: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    info!(
        concurrency,
        heartbeat_secs = lease.heartbeat.as_secs(),
        "MAF worker started, consumer={consumer}, stream={STREAM_KEY}"
    );

    loop {
        // A finished task still counts in `jobs.len()` until it is joined.
        while let Some(result) = jobs.try_join_next() {
            log_if_panicked(result);
        }
        let free = concurrency.saturating_sub(jobs.len());
        if free == 0 {
            // Every slot is busy: wait for one to free instead of reading. Not a
            // `select!` against the read below: dropping an XREADGROUP mid-flight
            // would still deliver its messages to this consumer, and nothing would
            // ever run them.
            if let Some(result) = jobs.join_next().await {
                log_if_panicked(result);
            }
            continue;
        }

        let (messages, reclaiming) = match recovery.take_call(lease.pass_every()) {
            Some(cursor) => {
                let (next, messages) = claim_abandoned(&mut reader, &lease, &cursor, free).await;
                recovery.finish_call(next);
                (messages, true)
            }
            None => match read_new(&mut reader, consumer, free).await {
                Ok(messages) => (messages, false),
                Err(e) => {
                    error!("MAF worker XREADGROUP error: {e}");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            },
        };

        for (msg_id, fields) in messages {
            let Some(mut job) = parse_job(&fields) else {
                // Malformed message — ACK to remove from PEL so it doesn't retry forever
                warn!("MAF worker: could not parse job from message {msg_id}, discarding");
                ack(&mut reader, &msg_id).await;
                continue;
            };
            if reclaiming {
                // Counted only for a recovered message: a fresh read is its first delivery.
                // Unreadable counts as within the cap, so a Redis hiccup never fails a run.
                job.deliveries = deliveries(&mut reader, &msg_id).await.unwrap_or(1);
                info!(
                    deliveries = job.deliveries,
                    "Reclaiming MAF execution {} from a stopped worker", job.execution_id
                );
            }
            let deps = deps.clone();
            let lease = lease.clone();
            let mut conn = jobs_conn.clone();
            jobs.spawn(async move { process_job(job, &msg_id, &deps, &mut conn, &lease).await });
        }
    }
}

/// A job task that panicked never acknowledged its message, and its heartbeat stopped with
/// it, so recovery takes the message over. `process_job` already turns an executor panic
/// into a recorded failure; this only catches one from the bookkeeping around it.
fn log_if_panicked(result: Result<(), tokio::task::JoinError>) {
    if let Err(e) = result
        && e.is_panic()
    {
        error!("MAF worker: job task panicked; recovery will take its message over");
    }
}

/// How many times `msg_id` has been delivered: once when first read, plus once for each
/// recovery that took it over. The heartbeat's refresh doesn't count (`REFRESH_OWNERSHIP`).
async fn deliveries(
    conn: &mut redis::aio::MultiplexedConnection,
    msg_id: &str,
) -> redis::RedisResult<i64> {
    let reply: redis::Value = redis::cmd("XPENDING")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg(msg_id)
        .arg(msg_id)
        .arg(1)
        .query_async(conn)
        .await?;
    Ok(parse_delivery_count(&reply).unwrap_or(1))
}

/// The delivery count in an extended XPENDING reply: `[[id, consumer, idle_ms, count]]`.
fn parse_delivery_count(reply: &redis::Value) -> Option<i64> {
    let redis::Value::Array(entries) = reply else {
        return None;
    };
    let redis::Value::Array(entry) = entries.first()? else {
        return None;
    };
    match entry.get(3)? {
        redis::Value::Int(count) => Some(*count),
        _ => None,
    }
}

/// Whether this worker still holds `msg_id`, refreshing its idle time if so.
async fn refresh_ownership(
    conn: &mut redis::aio::MultiplexedConnection,
    msg_id: &str,
    consumer: &str,
) -> redis::RedisResult<bool> {
    let still_held: i64 = REFRESH_OWNERSHIP
        .key(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg(msg_id)
        .arg(consumer)
        .invoke_async(conn)
        .await?;
    Ok(still_held == 1)
}

/// Refreshes `msg_id` every heartbeat while its run works, so recovery never takes it from
/// this live worker. Returns only once another worker has taken it over: this worker was
/// cut off long enough (`Lease::abandoned_after`) that recovery decided it had stopped.
async fn hold_message(mut conn: redis::aio::MultiplexedConnection, msg_id: &str, lease: &Lease) {
    let mut beat = tokio::time::interval(lease.heartbeat);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is immediate, and the message was only just delivered.
    beat.tick().await;
    loop {
        beat.tick().await;
        match refresh_ownership(&mut conn, msg_id, &lease.consumer).await {
            Ok(true) => {}
            Ok(false) => return,
            // An unreachable Redis is no sign that another worker has the message: keep
            // running and try again next beat.
            Err(e) => warn!("MAF worker: could not refresh message {msg_id}: {e}"),
        }
    }
}

/// Up to `count` new messages for this consumer, waiting at most 2 s for one to arrive.
async fn read_new(
    reader: &mut redis::aio::MultiplexedConnection,
    consumer: &str,
    count: usize,
) -> redis::RedisResult<Vec<(String, Vec<redis::Value>)>> {
    let result: redis::Value = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg(GROUP_NAME)
        .arg(consumer)
        .arg("BLOCK")
        .arg(2000u64)
        .arg("COUNT")
        .arg(count)
        .arg("STREAMS")
        .arg(STREAM_KEY)
        .arg(">")
        .query_async(reader)
        .await?;
    // Nil = block timeout, no messages.
    Ok(extract_messages(result))
}

struct Job {
    execution_id: Uuid,
    maf_json: String,
    user_id: Uuid,
    /// Run-time data for this execution only, spliced into step 0 by
    /// `executor::run_maf` — see `oss/server/src/maf.rs::RunWorkflowRequest`.
    content: Option<String>,
    /// Present only on a continuation job, `oss/server/src/hitl/mod.rs::deliver_maf`'s `XADD` —
    /// a fresh run always omits these three.
    resume: Option<ResumeFields>,
    /// How many times the stream has delivered this message (see `deliveries`): 1 for a
    /// fresh read, more for one recovery took over from a stopped worker.
    deliveries: i64,
}

struct ResumeFields {
    step_index: i32,
    task_id: String,
    answer: String,
}

fn parse_job(fields: &[redis::Value]) -> Option<Job> {
    let mut execution_id = None;
    let mut maf_json = None;
    let mut user_id = None;
    let mut content = None;
    let mut resume_step_index = None;
    let mut resume_task_id = None;
    let mut resume_answer = None;

    let mut i = 0;
    while i + 1 < fields.len() {
        // Use continue instead of ? so one malformed field doesn't drop the whole job
        let key = match bulk_str(&fields[i]) {
            Some(k) => k,
            None => {
                i += 2;
                continue;
            }
        };
        let val = match bulk_str(&fields[i + 1]) {
            Some(v) => v,
            None => {
                i += 2;
                continue;
            }
        };
        match key.as_str() {
            "execution_id" => execution_id = val.parse().ok(),
            "maf_json" => maf_json = Some(val),
            "user_id" => user_id = val.parse().ok(),
            "content" => content = Some(val),
            "resume_step_index" => resume_step_index = val.parse().ok(),
            "resume_task_id" => resume_task_id = Some(val),
            "resume_answer" => resume_answer = Some(val),
            _ => {}
        }
        i += 2;
    }

    // A fresh run's message never carries any of these three fields — `None` is the normal case.
    // But `deliver_maf`'s XADD always sends all three together for a continuation, so seeing *some*
    // of them present with even one failing to parse means this message was meant to be a resume,
    // not a fresh run. Falling through to `None` here previously downgraded it into a fresh run
    // from step 0 instead — silently discarding the human's answer and re-invoking every
    // already-succeeded step. Reject the whole job instead, matching the caller's existing
    // malformed-message handling (ACK + discard) rather than mis-executing it.
    let any_resume_field_present =
        resume_step_index.is_some() || resume_task_id.is_some() || resume_answer.is_some();
    let resume = match (resume_step_index, resume_task_id, resume_answer) {
        (Some(step_index), Some(task_id), Some(answer)) => Some(ResumeFields {
            step_index,
            task_id,
            answer,
        }),
        _ if any_resume_field_present => return None,
        _ => None,
    };

    Some(Job {
        execution_id: execution_id?,
        maf_json: maf_json?,
        user_id: user_id?,
        content,
        resume,
        deliveries: 1,
    })
}

fn bulk_str(val: &redis::Value) -> Option<String> {
    match val {
        redis::Value::BulkString(b) => String::from_utf8(b.clone()).ok(),
        redis::Value::SimpleString(s) => Some(s.clone()),
        _ => None,
    }
}

// XREADGROUP returns: Array([Array([stream_key, Array([Array([msg_id, Array([k,v,...])])])])])
fn extract_messages(val: redis::Value) -> Vec<(String, Vec<redis::Value>)> {
    let outer = match val {
        redis::Value::Array(v) => v,
        _ => return vec![],
    };
    let stream_entry = match outer.into_iter().next() {
        Some(redis::Value::Array(v)) => v,
        _ => return vec![],
    };
    match stream_entry.into_iter().nth(1) {
        Some(redis::Value::Array(entries)) => parse_entries(entries),
        _ => vec![],
    }
}

/// Stream entries as `(msg_id, fields)`, skipping any that are malformed.
fn parse_entries(entries: Vec<redis::Value>) -> Vec<(String, Vec<redis::Value>)> {
    let mut result = vec![];
    for msg in entries {
        let mut parts = match msg {
            redis::Value::Array(p) if p.len() == 2 => p.into_iter(),
            _ => continue,
        };
        let msg_id = match parts.next().and_then(|v| bulk_str(&v)) {
            Some(id) => id,
            None => continue,
        };
        let fields = match parts.next() {
            Some(redis::Value::Array(f)) => f,
            _ => continue,
        };
        result.push((msg_id, fields));
    }
    result
}

async fn process_job(
    job: Job,
    msg_id: &str,
    deps: &JobDeps,
    conn: &mut redis::aio::MultiplexedConnection,
    lease: &Lease,
) {
    let JobDeps {
        db,
        agent_calls,
        flow_guard,
        llm,
        hitl_store,
        maf_executor,
    } = deps;
    let execution_id = job.execution_id;
    let user_id = job.user_id;
    let content = job.content.clone();
    let maf_json_str = job.maf_json;
    let is_resume = job.resume.is_some();

    let (new_attempt, max_attempts) = match claim(db, execution_id, is_resume).await {
        Ok(Claim::Run {
            attempt,
            max_attempts,
        }) => (attempt, max_attempts),
        Ok(Claim::Stale(why)) => {
            warn!("MAF execution {execution_id}: message {msg_id} is stale ({why}), discarding");
            ack(conn, msg_id).await;
            return;
        }
        Err(e) => {
            error!("MAF worker: DB error claiming execution {execution_id}: {e}");
            return;
        }
    };

    if job.deliveries > MAX_DELIVERIES {
        let stops = job.deliveries - 1;
        let reason =
            format!("the worker running it stopped {stops} times while running it; not retrying");
        mark_failed(db, execution_id, &reason).await;
        ack(conn, msg_id).await;
        warn!("MAF execution {execution_id}: {reason}");
        return;
    }

    let maf_def: MafDefinition = match serde_json::from_str(&maf_json_str) {
        Ok(d) => d,
        Err(e) => {
            let err = format!("invalid maf_json: {e}");
            mark_failed(db, execution_id, &err).await;
            ack(conn, msg_id).await;
            return;
        }
    };

    // Caught rather than left to unwind past `process_job`. The job runs in its own task, so a
    // panic here (an out-of-bounds index reached some other way, a future bug) can no longer take
    // the worker down — but uncaught it would end the task before `finish_job`, leaving the
    // execution `running` and its message pending until the next restart. Caught, it becomes an
    // ordinary failure: retried or marked failed, and acknowledged. `AssertUnwindSafe` is safe
    // here: nothing on this stack is shared mutable state another task could observe
    // half-updated after a panic — everything panic could interrupt is either `Copy` (`db`,
    // `execution_id`, `user_id`), a fresh owned value about to be dropped and replaced with an
    // `Err`, or (`&mut conn`, this task's own) untouched by the executor itself.
    let ctx = MafRunContext {
        client: &agent_calls.client,
        agent_calls: &agent_calls.meter,
        outbound: agent_calls.outbound.as_ref(),
        db,
        flow_guard,
        llm,
        execution_id,
        user_id,
    };
    let run = std::panic::AssertUnwindSafe(async {
        match job.resume {
            None => maf_executor.run(&ctx, &maf_def, content.as_deref()).await,
            Some(resume) => match fetch_resume_state(db, execution_id).await {
                Ok((step_results, tokens_used, cost_used, output_generation)) => {
                    let resume = MafResume {
                        step_results,
                        tokens_used,
                        cost_used,
                        output_generation,
                        step_index: resume.step_index as usize,
                        task_id: resume.task_id,
                        context_id: execution_id.to_string(),
                        answer: resume.answer,
                    };
                    maf_executor.resume(&ctx, &maf_def, resume).await
                }
                Err(e) => Err(format!("failed to load resume state: {e}")),
            },
        }
    })
    .catch_unwind();
    // The run and its heartbeat together. `hold_message` returns only when another worker
    // has taken the message over, and the run is then dropped where it is (an agent or LLM
    // call in flight is cancelled with it): the new owner runs it, and this worker writes
    // nothing more and acknowledges nothing.
    let outcome = tokio::select! {
        outcome = run => outcome.unwrap_or_else(|_| {
            error!("MAF execution {execution_id}: executor panicked");
            Err("MAF executor panicked".to_string())
        }),
        () = hold_message(conn.clone(), msg_id, lease) => {
            warn!("MAF execution {execution_id}: another worker took over message {msg_id}; stopping this run");
            return;
        }
    };
    // The takeover can also land between the last heartbeat and here. Recording this run's
    // outcome then would overwrite the new owner's run, so check once more. A Redis error
    // can't tell, and the outcome is recorded as before.
    if let Ok(false) = refresh_ownership(conn, msg_id, &lease.consumer).await {
        warn!(
            "MAF execution {execution_id}: another worker took over message {msg_id}; not recording this run's outcome"
        );
        return;
    }

    finish_job(
        outcome,
        execution_id,
        msg_id,
        conn,
        db,
        hitl_store,
        user_id,
        &maf_json_str,
        content.as_deref(),
        new_attempt,
        max_attempts,
        &maf_def,
        is_resume,
    )
    .await;
}

/// What an execution's row says about running a message for it now.
enum Claim {
    /// Marked `running` with this attempt counted: run it.
    Run { attempt: i32, max_attempts: i32 },
    /// Nothing to run: no such execution, or one already past where this message
    /// could take it. Acknowledge and drop the message.
    Stale(String),
}

/// The statuses a message may start or continue its execution from. A fresh run or a
/// retry starts from `pending`, or from `running` when the worker that had it stopped. A
/// resume also from `awaiting_human`: `oss/server/src/hitl/mod.rs::deliver_maf` queues
/// the resume before it moves the row to `pending`, so the job can arrive first.
///
/// Any other status means the run moved on after this message was queued: a worker
/// stopped between writing the outcome and acknowledging, or a fresh-run message
/// arrived for a run that has since paused. Running it again would redo agent calls
/// for a run that already finished.
fn claimable_statuses(is_resume: bool) -> &'static [&'static str] {
    if is_resume {
        &["pending", "running", nasiko_types::maf::AWAITING_HUMAN]
    } else {
        &["pending", "running"]
    }
}

/// Marks the execution `running` and counts the attempt, if its status allows this
/// message to run it (`claimable_statuses`).
async fn claim(db: &PgPool, execution_id: Uuid, is_resume: bool) -> Result<Claim, sqlx::Error> {
    let claimed: Option<(i32, i32)> = sqlx::query_as(
        "UPDATE maf_executions \
         SET attempt_count = attempt_count + 1, status = 'running', started_at = now(), error = NULL \
         WHERE id = $1 AND status = ANY($2) \
         RETURNING attempt_count, max_attempts",
    )
    .bind(execution_id)
    .bind(claimable_statuses(is_resume))
    .fetch_optional(db)
    .await?;
    if let Some((attempt, max_attempts)) = claimed {
        return Ok(Claim::Run {
            attempt,
            max_attempts,
        });
    }

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM maf_executions WHERE id = $1")
            .bind(execution_id)
            .fetch_optional(db)
            .await?;
    Ok(Claim::Stale(match status {
        Some(status) => format!("the execution is already {status}"),
        None => "the execution was not found".to_string(),
    }))
}

/// Loads the durable state a resume needs from `maf_executions`: `step_results` (every step's
/// plan is embedded in it — see `run_maf_from`'s doc comment), the running token/cost totals, and
/// the planner's `output_generation` guideline (`oss/migrations/0024_maf_hitl.sql`,
/// `0012_maf_finops.sql`). Without `cost_usd` here, a resumed execution's FinOps total would reset
/// to whatever the post-resume steps alone cost, silently losing every pre-pause step's spend.
async fn fetch_resume_state(
    db: &PgPool,
    execution_id: Uuid,
) -> Result<(Vec<StepResult>, i64, f64, String), String> {
    #[derive(sqlx::FromRow)]
    struct ResumeStateRow {
        step_results: Option<String>,
        tokens_used: i64,
        cost_usd: f64,
        output_generation: Option<String>,
    }

    let row = sqlx::query_as::<_, ResumeStateRow>(
        "SELECT step_results::text AS step_results, tokens_used, cost_usd, output_generation \
         FROM maf_executions WHERE id = $1",
    )
    .bind(execution_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "execution not found".to_string())?;

    let step_results: Vec<StepResult> = match row.step_results.as_deref() {
        Some(s) => serde_json::from_str(s).map_err(|e| format!("invalid step_results: {e}"))?,
        None => return Err("execution has no step_results to resume from".to_string()),
    };

    Ok((
        step_results,
        row.tokens_used,
        row.cost_usd,
        row.output_generation.unwrap_or_default(),
    ))
}

/// Handles a MAF run's outcome — success, a fresh pause, or an error — shared by both a normal
/// run and a resumed one so the two paths never diverge in how they finish.
#[allow(clippy::too_many_arguments)]
async fn finish_job(
    outcome: Result<StepOutcome, String>,
    execution_id: Uuid,
    msg_id: &str,
    conn: &mut redis::aio::MultiplexedConnection,
    db: &PgPool,
    hitl_store: &Arc<dyn HitlStore>,
    user_id: Uuid,
    maf_json_str: &str,
    // The original run's content, carried forward so a retry's re-enqueue keeps it.
    content: Option<&str>,
    new_attempt: i32,
    max_attempts: i32,
    maf_def: &MafDefinition,
    is_resume: bool,
) {
    match outcome {
        Ok(StepOutcome::Completed(result)) => {
            let step_json = serde_json::to_value(&result.step_results).unwrap_or_default();
            let step_json_str = step_json.to_string();
            let _ = sqlx::query(
                r#"UPDATE maf_executions
                   SET status = 'success',
                       output = $1,
                       step_results = $2::jsonb,
                       tokens_used = $3,
                       cost_usd = $4,
                       completed_at = now(),
                       duration_ms = EXTRACT(EPOCH FROM (now() - started_at))::BIGINT * 1000
                   WHERE id = $5"#,
            )
            .bind(&result.output)
            .bind(&step_json_str)
            .bind(result.tokens_used)
            .bind(result.cost_usd)
            .bind(execution_id)
            .execute(db)
            .await;
            ack(conn, msg_id).await;
            info!("MAF execution {execution_id} succeeded");
        }
        Ok(StepOutcome::AwaitingHuman(paused)) => {
            // `step_results`/`tokens_used` were already persisted by the executor at the moment
            // it detected the pause (`execute_step`/`continue_paused_step`'s own
            // `persist_progress` call) — only the execution-level status still needs flipping.
            let _ = sqlx::query("UPDATE maf_executions SET status = $1 WHERE id = $2")
                .bind(nasiko_types::maf::AWAITING_HUMAN)
                .bind(execution_id)
                .execute(db)
                .await;
            let step_index = paused.step_index;
            match create_hitl_request(hitl_store, execution_id, user_id, maf_def, &paused).await {
                Ok(()) => {
                    // Done for now — no retry, no re-enqueue. The row waits for
                    // `POST /api/hitl/{id}/resolve` to trigger a continuation job.
                    ack(conn, msg_id).await;
                    info!(
                        "MAF execution {execution_id} paused at step {step_index} awaiting a human"
                    );
                }
                Err(e) => {
                    // No `hitl_requests` row exists for a human to ever resolve — leaving the
                    // execution at `awaiting_human` would strand it forever with nothing pointing
                    // back to it once this message is ACKed. Fail it outright instead: visible
                    // and re-runnable, unlike a silent, permanently unresolvable pause.
                    mark_failed(
                        db,
                        execution_id,
                        &format!(
                            "paused at step {step_index} but failed to record the HITL request: {e}"
                        ),
                    )
                    .await;
                    ack(conn, msg_id).await;
                    warn!(
                        "MAF execution {execution_id} paused at step {step_index} but HITL request creation failed, marked failed: {e}"
                    );
                }
            }
        }
        Err(e) => {
            // A resumed job's failure is never retried by restarting from step 0 — that would
            // silently redo already-succeeded steps' agent calls and discard the human's answer.
            // Terminal failure here, not a re-enqueue, is the safe default (resume-retry
            // semantics aren't specified yet; hardening that is a follow-up, not required
            // for this phase).
            if is_resume || new_attempt >= max_attempts {
                mark_failed(db, execution_id, &e).await;
                ack(conn, msg_id).await;
                warn!(
                    "MAF execution {execution_id} terminal failure after {new_attempt} attempt(s): {e}"
                );
            } else {
                // Reset to pending and re-enqueue for retry
                let _ = sqlx::query(
                    "UPDATE maf_executions SET status = 'pending', error = $1 WHERE id = $2",
                )
                .bind(&e)
                .bind(execution_id)
                .execute(db)
                .await;
                re_enqueue(conn, execution_id, maf_json_str, user_id, content).await;
                ack(conn, msg_id).await;
                warn!(
                    "MAF execution {execution_id} failed (attempt {new_attempt}/{max_attempts}), re-enqueued: {e}"
                );
            }
        }
    }
}

/// Creates the `hitl_requests` row a paused step surfaces to a human, resolved via the generic
/// `POST /api/hitl/{id}/resolve` (`oss/server/src/router/hitl.rs` — already origin-agnostic).
///
/// Returns `Err` rather than only logging on failure: the caller (`finish_job`) has already set
/// `maf_executions.status = 'awaiting_human'` and is about to ACK the Redis message regardless —
/// if this row never gets created, the execution is stuck in that status forever with nothing a
/// human can ever resolve, and no later sweep catches it (the message is gone from the stream).
/// The caller uses a failed create to fail the execution outright instead, which is at least
/// visible and re-runnable, rather than silently unresolvable.
async fn create_hitl_request(
    hitl_store: &Arc<dyn HitlStore>,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    paused: &PausedStep,
) -> Result<(), String> {
    let Some(step) = maf_def.steps.get(paused.step_index as usize) else {
        let err = format!(
            "paused step index {} out of range, cannot record the HITL pause",
            paused.step_index
        );
        error!("MAF execution {execution_id}: {err}");
        return Err(err);
    };
    let req = NewHitlRequest::maf(
        paused.kind,
        step.agent_id,
        user_id,
        paused.task_id.clone(),
        paused.context_id.clone(),
        execution_id,
        paused.step_index,
        paused.question.clone(),
    );
    hitl_store.create(req).await.map(|_| ()).map_err(|e| {
        let err = format!("failed to persist HITL pause: {e}");
        error!("MAF execution {execution_id}: {err}");
        err
    })
}

async fn mark_failed(db: &PgPool, execution_id: Uuid, error: &str) {
    // A failed run still spent real money — planning and any completed steps
    // all made billed LLM calls before the failure. Recording 0 here would
    // under-report spend on exactly the runs that are most likely to be
    // retried, and the retries would compound it. The platform already has
    // the priced rows; read them back the same way a successful run does.
    let spend = executor::platform_spend(db, execution_id).await;

    let _ = sqlx::query(
        r#"UPDATE maf_executions
           SET status = 'failed',
               error = $1,
               tokens_used = $2,
               cost_usd = $3,
               completed_at = now(),
               duration_ms = EXTRACT(EPOCH FROM (now() - COALESCE(started_at, now())))::BIGINT * 1000
           WHERE id = $4"#,
    )
    .bind(error)
    .bind(spend.total_tokens)
    .bind(spend.total_cost_usd)
    .bind(execution_id)
    .execute(db)
    .await;
}

async fn ack(conn: &mut redis::aio::MultiplexedConnection, msg_id: &str) {
    let _: redis::RedisResult<()> = redis::cmd("XACK")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg(msg_id)
        .query_async(conn)
        .await;
}

async fn re_enqueue(
    conn: &mut redis::aio::MultiplexedConnection,
    execution_id: Uuid,
    maf_json: &str,
    user_id: Uuid,
    content: Option<&str>,
) {
    let mut xadd = redis::cmd("XADD");
    xadd.arg(STREAM_KEY)
        .arg("*")
        .arg("execution_id")
        .arg(execution_id.to_string())
        .arg("maf_json")
        .arg(maf_json)
        .arg("user_id")
        .arg(user_id.to_string());
    // Carry the original run's content forward on retry — otherwise a
    // content-bearing execution would silently lose it on its 2nd/3rd attempt.
    if let Some(content) = content {
        xadd.arg("content").arg(content);
    }
    let _: redis::RedisResult<String> = xadd.query_async(conn).await;
}

/// One batch of up to `count` messages left untouched longer than `Lease::abandoned_after`
/// (their worker stopped mid-run), claimed for this consumer, and the cursor the next batch
/// continues from: `None` once the scan has reached the end of the pending list. Each
/// claim adds one to the message's delivery count.
async fn claim_abandoned(
    reader: &mut redis::aio::MultiplexedConnection,
    lease: &Lease,
    cursor: &str,
    count: usize,
) -> (Option<String>, Vec<(String, Vec<redis::Value>)>) {
    let min_idle_ms = u64::try_from(lease.abandoned_after().as_millis()).unwrap_or(u64::MAX);
    let result: redis::RedisResult<redis::Value> = redis::cmd("XAUTOCLAIM")
        .arg(STREAM_KEY)
        .arg(GROUP_NAME)
        .arg(&lease.consumer)
        .arg(min_idle_ms)
        .arg(cursor)
        .arg("COUNT")
        .arg(count)
        .query_async(reader)
        .await;
    match result {
        Ok(reply) => parse_autoclaim(reply),
        Err(e) => {
            // Stream or group may not exist yet on first boot — not an error
            warn!("MAF worker XAUTOCLAIM skipped (stream may be new): {e}");
            (None, Vec::new())
        }
    }
}

/// XAUTOCLAIM returns `[next_cursor, [[msg_id, fields], ...], [deleted_ids]]`; a next cursor of
/// `0-0` means the scan is complete.
fn parse_autoclaim(reply: redis::Value) -> (Option<String>, Vec<(String, Vec<redis::Value>)>) {
    let redis::Value::Array(parts) = reply else {
        return (None, Vec::new());
    };
    let mut parts = parts.into_iter();
    let next = parts
        .next()
        .and_then(|v| bulk_str(&v))
        .filter(|cursor| cursor != "0-0");
    match parts.next() {
        Some(redis::Value::Array(entries)) => (next, parse_entries(entries)),
        _ => (None, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Value;

    fn text(s: &str) -> Value {
        Value::BulkString(s.as_bytes().to_vec())
    }

    fn entry(id: &str) -> Value {
        Value::Array(vec![
            text(id),
            Value::Array(vec![text("execution_id"), text("e")]),
        ])
    }

    #[test]
    fn an_autoclaim_batch_carries_its_entries_and_the_next_cursor() {
        let reply = Value::Array(vec![
            text("1700000000000-3"),
            Value::Array(vec![entry("1-0"), entry("2-0")]),
            Value::Array(vec![]),
        ]);
        let (next, messages) = parse_autoclaim(reply);
        assert_eq!(next.as_deref(), Some("1700000000000-3"));
        let ids: Vec<&str> = messages.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["1-0", "2-0"]);
    }

    #[test]
    fn an_autoclaim_cursor_of_zero_ends_the_scan() {
        let reply = Value::Array(vec![
            text("0-0"),
            Value::Array(vec![entry("1-0")]),
            Value::Array(vec![]),
        ]);
        let (next, messages) = parse_autoclaim(reply);
        assert_eq!(next, None, "the pending list has been scanned to its end");
        assert_eq!(messages.len(), 1, "the last batch is still returned");
    }

    #[test]
    fn a_malformed_autoclaim_reply_ends_the_scan_with_nothing() {
        let (next, messages) = parse_autoclaim(Value::Nil);
        assert_eq!(next, None);
        assert!(messages.is_empty());
    }

    #[test]
    fn a_read_that_timed_out_has_no_messages() {
        assert!(extract_messages(Value::Nil).is_empty());
    }

    /// A pass that is never due again on its own, so a test decides when the next starts.
    const NOT_YET: Duration = Duration::from_secs(3600);

    #[test]
    fn the_startup_pass_runs_first_and_ends_at_the_end_of_the_list() {
        let mut recovery = Recovery::at_startup();
        assert_eq!(recovery.take_call(NOT_YET).as_deref(), Some("0-0"));
        recovery.finish_call(Some("5-0".into()));
        assert_eq!(
            recovery.take_call(NOT_YET).as_deref(),
            Some("5-0"),
            "a pass continues from where its last call stopped"
        );
        recovery.finish_call(None);
        assert_eq!(
            recovery.take_call(NOT_YET),
            None,
            "the pass is over and the next isn't due"
        );
    }

    #[test]
    fn a_new_pass_starts_from_the_beginning_once_due() {
        let mut recovery = Recovery::at_startup();
        recovery.take_call(NOT_YET);
        recovery.finish_call(None);
        assert_eq!(recovery.take_call(Duration::ZERO).as_deref(), Some("0-0"));
    }

    #[test]
    fn a_pass_out_of_calls_stops_and_the_next_continues_where_it_stopped() {
        let mut recovery = Recovery::at_startup();
        for call in 0..CLAIM_CALLS_PER_PASS {
            assert!(
                recovery.take_call(NOT_YET).is_some(),
                "call {call} is within the budget"
            );
            recovery.finish_call(Some(format!("{call}-1")));
        }
        assert_eq!(
            recovery.take_call(NOT_YET),
            None,
            "the budget is spent: new work is read until the next pass"
        );
        let resumed = recovery.take_call(Duration::ZERO);
        assert_eq!(
            resumed.as_deref(),
            Some(format!("{}-1", CLAIM_CALLS_PER_PASS - 1).as_str()),
            "the next pass continues from the saved cursor, not the beginning"
        );
    }

    #[test]
    fn the_delivery_count_is_read_from_an_xpending_entry() {
        let reply = Value::Array(vec![Value::Array(vec![
            text("1-0"),
            text("maf-worker-a"),
            Value::Int(130_000),
            Value::Int(4),
        ])]);
        assert_eq!(parse_delivery_count(&reply), Some(4));
        assert_eq!(
            parse_delivery_count(&Value::Array(vec![])),
            None,
            "no longer pending"
        );
    }

    #[test]
    fn malformed_entries_are_skipped() {
        let entries = vec![entry("1-0"), Value::Array(vec![text("2-0")]), text("junk")];
        let ids: Vec<String> = parse_entries(entries)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["1-0"]);
    }
}
