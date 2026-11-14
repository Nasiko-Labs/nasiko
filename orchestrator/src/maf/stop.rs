//! Stopping MAF executions on their owner's request.
//!
//! A stop is decided in the database: [`stop_in_db`] moves each execution still in flight to
//! [`STOPPED`] in one transaction, which every later write by a worker respects (they all skip a
//! stopped row). What is left is telling the worker running an execution to stop now rather
//! than at its next write:
//!
//! - A `pending` execution has no worker yet. The worker that later reads its message finds it
//!   stopped when it claims it, and discards the message without running anything.
//! - An `awaiting_human` execution has no worker either, and no message: its open HITL requests
//!   are canceled in the same transaction, and a late answer finds nothing to resume.
//! - A `running` execution is held by one worker. [`notify_owners`] finds that worker in the
//!   stream's pending list and publishes the execution id on the worker's own channel; its
//!   [`listen`] task cuts the run short through the process-local record of its runs.
//!
//! A message can be lost (Pub/Sub keeps nothing for a subscriber that is reconnecting). The run
//! then stops at its next progress write, which finds the row stopped (`executor::persist_progress`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use futures::StreamExt;
use nasiko_types::maf::{AWAITING_HUMAN, STOPPED};
use sqlx::PgPool;
use tokio::sync::{Notify, watch};
use tracing::{info, warn};
use uuid::Uuid;

use super::worker::{GROUP_NAME, STREAM_KEY};

/// Each worker listens on `<prefix><its consumer name>`, so a stop reaches only the worker
/// running the execution.
const CHANNEL_PREFIX: &str = "nasiko:maf:stop:";

/// Execution ids per published message, so a bulk stop never builds one huge payload.
const IDS_PER_MESSAGE: usize = 500;

/// The statuses a stop applies to: everything not yet finished.
const STOPPABLE: [&str; 3] = ["pending", "running", AWAITING_HUMAN];

/// How long the listener waits before reconnecting, doubling up to [`MAX_RECONNECT_WAIT`].
const FIRST_RECONNECT_WAIT: Duration = Duration::from_secs(1);
const MAX_RECONNECT_WAIT: Duration = Duration::from_secs(30);

fn channel(consumer: &str) -> String {
    format!("{CHANNEL_PREFIX}{consumer}")
}

// ─── The stop request ─────────────────────────────────────────────────────────

/// Which of the caller's executions a stop applies to.
#[derive(Clone, Copy)]
pub enum StopScope<'a> {
    /// These executions; ids the caller doesn't own are reported as not found.
    Executions(&'a [Uuid]),
    /// Every unfinished execution of this workflow.
    Workflow(Uuid),
}

/// What a stop did, per execution.
#[derive(Debug, Default)]
pub struct StopReport {
    /// Stopped by this request, with the status each had.
    pub stopped: Vec<(Uuid, String)>,
    /// Asked for by id but already finished, with the status each ended in.
    pub already_finished: Vec<(Uuid, String)>,
    /// Asked for by id but not one of the caller's executions.
    pub not_found: Vec<Uuid>,
}

impl StopReport {
    /// The executions a worker was running when they were stopped: the ones to notify.
    pub fn were_running(&self) -> Vec<Uuid> {
        self.stopped
            .iter()
            .filter(|(_, previous)| previous == "running")
            .map(|(id, _)| *id)
            .collect()
    }
}

/// Stops `owner`'s unfinished executions in `scope`, in one transaction: each becomes
/// [`STOPPED`] (with `completed_at`, and every step that hadn't finished marked stopped too),
/// and the open HITL requests of the paused ones are canceled.
///
/// The owner is the user who ran the execution, which is always the workflow's owner: only the
/// owner can run a workflow (`run_workflow`). Checked on the execution itself, like every other
/// execution endpoint, so a run whose workflow is gone can still be stopped.
///
/// Each row is locked before it is changed, so a worker claiming or finishing it at the same
/// moment is ordered before or after the stop, never interleaved: either the stop sees
/// `running` (and the worker is notified) or the worker's claim finds the row stopped. Rows are
/// locked in id order, so two stops over the same runs never deadlock.
///
/// The run's spend so far is read back from the platform's records (`trace_usage`, the same
/// source as `executor::platform_spend`) and written on the row: a run's progress writes carry
/// no cost, and a stopped run that no worker holds (queued, paused, or its worker gone) has no
/// one else to record it. A running run's worker overwrites it once its last usage writes land.
pub async fn stop_in_db(
    db: &PgPool,
    owner: Uuid,
    scope: StopScope<'_>,
) -> Result<StopReport, sqlx::Error> {
    let (ids, workflow_id): (&[Uuid], Option<Uuid>) = match scope {
        StopScope::Executions(ids) => (ids, None),
        StopScope::Workflow(id) => (&[], Some(id)),
    };

    let mut tx = db.begin().await?;
    let stopped: Vec<(Uuid, String)> = sqlx::query_as(
        r#"WITH target AS (
               SELECT e.id, e.status
                 FROM maf_executions e
                WHERE e.user_id = $1
                  AND (e.id = ANY($2) OR e.maf_id = $3)
                  AND e.status = ANY($4)
                ORDER BY e.id
                  FOR UPDATE OF e
           ),
           spend AS (
               SELECT t.id,
                      COUNT(u.trace_id) AS calls,
                      COALESCE(SUM(u.input_tokens + u.output_tokens + u.cache_read_tokens), 0)::BIGINT AS tokens,
                      COALESCE(SUM(u.cost_usd), 0)::DOUBLE PRECISION AS cost
                 FROM target t
                 LEFT JOIN trace_usage u
                   ON u.trace_id = t.id::text
                  AND (u.agent_name = 'maf' OR u.agent_name LIKE 'maf:step%')
                GROUP BY t.id
           )
           UPDATE maf_executions e
              SET status = $5,
                  completed_at = now(),
                  duration_ms = EXTRACT(EPOCH FROM (now() - COALESCE(e.started_at, now())))::BIGINT * 1000,
                  tokens_used = CASE WHEN s.calls > 0 THEN s.tokens ELSE e.tokens_used END,
                  cost_usd = CASE WHEN s.calls > 0 THEN s.cost ELSE e.cost_usd END,
                  step_results = CASE
                      WHEN jsonb_typeof(e.step_results) = 'array' THEN (
                          SELECT COALESCE(jsonb_agg(
                                     CASE WHEN s->>'status' = ANY($4)
                                          THEN jsonb_set(s, '{status}', to_jsonb($5::text))
                                          ELSE s END
                                     ORDER BY n), '[]'::jsonb)
                            FROM jsonb_array_elements(e.step_results) WITH ORDINALITY AS x(s, n))
                      ELSE e.step_results
                  END
             FROM target t
             JOIN spend s ON s.id = t.id
            WHERE e.id = t.id
        RETURNING e.id, t.status"#,
    )
    .bind(owner)
    .bind(ids)
    .bind(workflow_id)
    .bind(&STOPPABLE[..])
    .bind(STOPPED)
    .fetch_all(&mut *tx)
    .await?;

    let stopped_ids: Vec<Uuid> = stopped.iter().map(|(id, _)| *id).collect();
    sqlx::query(
        "UPDATE hitl_requests
            SET status = 'canceled', resolved_by = $1, resolved_at = now()
          WHERE maf_execution_id = ANY($2) AND status = 'pending'",
    )
    .bind(owner)
    .bind(&stopped_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut report = StopReport {
        stopped,
        ..StopReport::default()
    };
    let unmatched: Vec<Uuid> = ids
        .iter()
        .filter(|id| !stopped_ids.contains(id))
        .copied()
        .collect();
    if !unmatched.is_empty() {
        let finished: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, status FROM maf_executions WHERE user_id = $1 AND id = ANY($2)",
        )
        .bind(owner)
        .bind(&unmatched)
        .fetch_all(db)
        .await?;
        report.not_found = unmatched
            .iter()
            .filter(|id| !finished.iter().any(|(found, _)| found == *id))
            .copied()
            .collect();
        report.already_finished = finished;
    }
    Ok(report)
}

// ─── Telling the owning worker ────────────────────────────────────────────────

/// Atomically: of the stream's pending messages (each delivered to a worker that hasn't
/// acknowledged it yet), the ones whose `execution_id` is among `ARGV[2..]`, as a flat list of
/// `consumer, execution_id` pairs. Only those two strings leave Redis, never a message's
/// `maf_json`. The pending list holds only runs in flight, so the scan is as long as the number
/// of executions running across all workers.
static FIND_OWNERS: LazyLock<redis::Script> = LazyLock::new(|| {
    redis::Script::new(
        r"
        local wanted = {}
        for i = 2, #ARGV do wanted[ARGV[i]] = true end
        local found = {}
        local start = '-'
        while true do
            local pending = redis.call('XPENDING', KEYS[1], ARGV[1], start, '+', 1000)
            for _, entry in ipairs(pending) do
                local message = redis.call('XRANGE', KEYS[1], entry[1], entry[1])[1]
                if message then
                    local fields = message[2]
                    for j = 1, #fields - 1, 2 do
                        if fields[j] == 'execution_id' then
                            if wanted[fields[j + 1]] then
                                table.insert(found, entry[2])
                                table.insert(found, fields[j + 1])
                            end
                            break
                        end
                    end
                end
            end
            if #pending < 1000 then break end
            start = '(' .. pending[#pending][1]
        end
        return found
        ",
    )
});

/// Tells the worker running each of `execution_ids` to stop it now: finds each one's worker
/// in the stream's pending list and publishes the ids on that worker's channel, one message per
/// worker (more for very large stops). Returns how many workers were messaged.
///
/// An id no worker holds is skipped: its run has ended, or its worker is gone and the run will
/// be found stopped by whichever worker recovers it. An execution held by two workers (a
/// takeover still settling) is sent to both.
pub async fn notify_owners<C>(conn: &mut C, execution_ids: &[Uuid]) -> redis::RedisResult<usize>
where
    C: redis::aio::ConnectionLike + Send,
{
    if execution_ids.is_empty() {
        return Ok(0);
    }
    let mut invocation = FIND_OWNERS.key(STREAM_KEY);
    invocation.arg(GROUP_NAME);
    for id in execution_ids {
        invocation.arg(id.to_string());
    }
    let pairs: Vec<String> = invocation.invoke_async(conn).await?;

    let owners = by_owner(&pairs);
    for (consumer, ids) in &owners {
        for chunk in ids.chunks(IDS_PER_MESSAGE) {
            let _: i64 = redis::cmd("PUBLISH")
                .arg(channel(consumer))
                .arg(chunk.join(","))
                .query_async(conn)
                .await?;
        }
    }
    Ok(owners.len())
}

/// `[consumer, execution_id, consumer, execution_id, ...]` grouped by consumer.
fn by_owner(pairs: &[String]) -> HashMap<&str, Vec<&str>> {
    let mut owners: HashMap<&str, Vec<&str>> = HashMap::new();
    for [consumer, execution_id] in pairs.as_chunks::<2>().0 {
        owners
            .entry(consumer.as_str())
            .or_default()
            .push(execution_id.as_str());
    }
    owners
}

/// The execution ids in one stop message; anything that isn't one is ignored.
fn parse_ids(payload: &str) -> impl Iterator<Item = Uuid> + '_ {
    payload.split(',').filter_map(|id| id.trim().parse().ok())
}

/// Receives the stop messages meant for `consumer` and stops the runs they name, for as long
/// as the worker runs. Reconnects when the subscription drops; a message sent meanwhile is
/// lost, and that run stops at its next progress write instead.
pub(super) async fn listen(redis: redis::Client, consumer: String) {
    let channel = channel(&consumer);
    let mut wait = FIRST_RECONNECT_WAIT;
    loop {
        match subscribe(&redis, &channel).await {
            Ok(messages) => {
                wait = FIRST_RECONNECT_WAIT;
                let mut messages = std::pin::pin!(messages);
                while let Some(payload) = messages.next().await {
                    for execution_id in parse_ids(&payload) {
                        if stop_local(execution_id) {
                            info!("MAF execution {execution_id}: stop requested by its owner");
                        }
                    }
                }
                warn!("MAF worker: stop channel {channel} closed; reconnecting");
            }
            Err(e) => warn!("MAF worker: could not subscribe to {channel}: {e}"),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MAX_RECONNECT_WAIT);
    }
}

async fn subscribe(
    redis: &redis::Client,
    channel: &str,
) -> redis::RedisResult<impl futures::Stream<Item = String>> {
    let mut pubsub = redis.get_async_pubsub().await?;
    pubsub.subscribe(channel).await?;
    Ok(pubsub
        .into_on_message()
        .filter_map(|msg| async move { msg.get_payload::<String>().ok() }))
}

// ─── The runs this process is executing ───────────────────────────────────────

/// One execution this process is running: the signal that stops it and its usage writes in
/// flight. Shared by every job of this process running the same execution (two only while a
/// takeover settles), so one stop reaches them all.
struct LocalRun {
    stop: watch::Sender<bool>,
    usage_writes: Arc<UsageWrites>,
    jobs: usize,
}

/// Process-wide, like the worker it serves: a stop arrives on the worker's channel and from
/// the executor's progress write, neither of which holds the job that is running.
static LOCAL_RUNS: LazyLock<Mutex<HashMap<Uuid, LocalRun>>> = LazyLock::new(Default::default);

fn local_runs() -> MutexGuard<'static, HashMap<Uuid, LocalRun>> {
    // Nothing a panic could leave half-done is held under the lock.
    LOCAL_RUNS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registers a job as running `execution_id` for as long as the guard lives. Registered before
/// the job claims the execution, so a stop that lands between the claim and the run starting is
/// not missed.
pub(super) fn register(execution_id: Uuid) -> RunGuard {
    let mut runs = local_runs();
    let run = runs.entry(execution_id).or_insert_with(|| LocalRun {
        stop: watch::channel(false).0,
        usage_writes: Arc::default(),
        jobs: 0,
    });
    run.jobs += 1;
    RunGuard {
        execution_id,
        stopped: run.stop.subscribe(),
        usage_writes: run.usage_writes.clone(),
    }
}

/// Stops `execution_id` if this process is running it; whether it was.
pub(super) fn stop_local(execution_id: Uuid) -> bool {
    match local_runs().get(&execution_id) {
        Some(run) => {
            run.stop.send_replace(true);
            true
        }
        None => false,
    }
}

/// A job's hold on a [`LocalRun`].
pub(super) struct RunGuard {
    execution_id: Uuid,
    stopped: watch::Receiver<bool>,
    usage_writes: Arc<UsageWrites>,
}

impl RunGuard {
    /// Resolves once the execution is stopped; never otherwise.
    pub(super) async fn stopped(&self) {
        let mut stopped = self.stopped.clone();
        if stopped.wait_for(|stopped| *stopped).await.is_err() {
            // The sender lives as long as this guard, so this can't happen; never resolve.
            std::future::pending::<()>().await;
        }
    }

    /// Waits up to `limit` for the usage writes still in flight; whether they all finished.
    pub(super) async fn usage_written(&self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.usage_writes.settled())
            .await
            .is_ok()
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut runs = local_runs();
        if let Some(run) = runs.get_mut(&self.execution_id) {
            run.jobs -= 1;
            if run.jobs == 0 {
                runs.remove(&self.execution_id);
            }
        }
    }
}

// ─── Usage writes that outlive a stopped run ──────────────────────────────────

/// The usage writes in flight for one execution.
#[derive(Default)]
struct UsageWrites {
    in_flight: AtomicUsize,
    settled: Notify,
}

impl UsageWrites {
    async fn settled(&self) {
        loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.in_flight.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// Counts one write as in flight until it is dropped, however its task ends.
struct InFlight(Arc<UsageWrites>);

impl InFlight {
    fn begin(writes: Arc<UsageWrites>) -> Self {
        writes.in_flight.fetch_add(1, Ordering::AcqRel);
        Self(writes)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.0.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.settled.notify_waiters();
        }
    }
}

/// Runs a usage write for `execution_id` as its own task. A run that is stopped (or taken
/// over) is dropped wherever it is; a write dropped mid-transaction would roll back the record
/// of a call already paid for. As a task it finishes regardless, and a stopped run's cleanup
/// waits for it ([`RunGuard::usage_written`]) before it reads the run's spend.
pub(super) fn spawn_usage_write<F>(execution_id: Uuid, write: F) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let in_flight = local_runs()
        .get(&execution_id)
        .map(|run| InFlight::begin(run.usage_writes.clone()));
    tokio::spawn(async move {
        let _in_flight = in_flight;
        write.await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_are_grouped_by_the_worker_holding_them() {
        let pairs: Vec<String> = ["w1", "a", "w2", "b", "w1", "c"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let owners = by_owner(&pairs);
        assert_eq!(owners.len(), 2);
        assert_eq!(owners["w1"], ["a", "c"]);
        assert_eq!(owners["w2"], ["b"]);
    }

    #[test]
    fn a_stop_message_yields_only_valid_ids() {
        let id = Uuid::new_v4();
        let payload = format!("{id}, not-a-uuid,,{id}");
        let ids: Vec<Uuid> = parse_ids(&payload).collect();
        assert_eq!(ids, [id, id]);
    }

    #[test]
    fn only_runs_a_worker_held_are_notified() {
        let (running, pending) = (Uuid::new_v4(), Uuid::new_v4());
        let report = StopReport {
            stopped: vec![(running, "running".into()), (pending, "pending".into())],
            ..StopReport::default()
        };
        assert_eq!(report.were_running(), [running]);
    }

    #[tokio::test]
    async fn a_local_stop_reaches_every_job_running_the_execution() {
        let id = Uuid::new_v4();
        let first = register(id);
        let second = register(id);
        assert!(stop_local(id));
        first.stopped().await;
        second.stopped().await;
    }

    #[tokio::test]
    async fn a_job_registered_after_the_stop_sees_it() {
        let id = Uuid::new_v4();
        let first = register(id);
        stop_local(id);
        let late = register(id);
        late.stopped().await;
        drop(first);
    }

    #[test]
    fn the_last_job_to_finish_forgets_the_run() {
        let id = Uuid::new_v4();
        let first = register(id);
        let second = register(id);
        drop(first);
        assert!(stop_local(id), "still running in the second job");
        drop(second);
        assert!(!stop_local(id), "no job runs it any more");
    }

    #[tokio::test]
    async fn a_stopped_run_waits_for_its_usage_writes() {
        let id = Uuid::new_v4();
        let guard = register(id);
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let write = spawn_usage_write(id, async move {
            let _ = released.await;
        });
        assert!(
            !guard.usage_written(Duration::from_millis(20)).await,
            "the write is still in flight"
        );
        release.send(()).unwrap();
        write.await.unwrap();
        assert!(guard.usage_written(Duration::from_millis(20)).await);
    }

    #[tokio::test]
    async fn a_write_for_a_run_not_held_here_still_runs() {
        let (done, finished) = tokio::sync::oneshot::channel();
        spawn_usage_write(Uuid::new_v4(), async move {
            let _ = done.send(());
        })
        .await
        .unwrap();
        assert!(finished.await.is_ok());
    }
}
