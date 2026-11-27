# Build Worker

`oss/server/src/agents/build_worker.rs`

A Postgres-backed, in-process job queue that runs all agent build operations asynchronously.
HTTP handlers enqueue jobs and return immediately; one background task (per server replica)
drains the queue.

---

## Why a job queue?

Agent builds — OCI image pushes, `git clone` + buildkit invocations, rollbacks — can take 5–30
minutes. Holding an HTTP connection open for that long is fragile (load-balancer timeouts,
client disconnects). The job queue decouples the HTTP request from the actual build, letting the
frontend poll for progress via the `agent_builds` status column.

The `build_jobs` table is the queue. Postgres `FOR UPDATE SKIP LOCKED` is the distributed lock.
No Redis, no Celery, no external broker.

---

## Database schema

```sql
CREATE TABLE build_jobs (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id     UUID        NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    owner_id     UUID        NOT NULL,
    payload      JSONB       NOT NULL,       -- BuildJobPayload (tagged enum)
    status       TEXT        NOT NULL DEFAULT 'pending'
                             CHECK (status IN ('pending','in_progress','done','failed')),
    attempt      INTEGER     NOT NULL DEFAULT 0,
    error_msg    TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    picked_at    TIMESTAMPTZ,               -- set when claimed; NULL = pending or done
    completed_at TIMESTAMPTZ                -- set when done or failed
);

-- Partial index covers only the statuses the worker queries.
CREATE INDEX build_jobs_work_queue ON build_jobs(status, created_at)
    WHERE status IN ('pending', 'in_progress');
```

`ON DELETE CASCADE` — when an agent is deleted mid-build, the job row disappears. The executor
handles the resulting no-op `UPDATE` gracefully (0 rows affected is fine).

---

## Job payload variants

`BuildJobPayload` is a `#[serde(tag = "kind")]` enum defined in `upload.rs`.
All fields are serialized to JSONB in the `payload` column.

| Variant | Trigger | Executor |
|---|---|---|
| `Upload` | New agent uploaded via zip | `execute_upload_and_deploy` in `upload.rs` |
| `Update` | Agent source updated (new version) | `execute_agent_update` in `update.rs` |
| `Rollback` | Agent rolled back to a previous version | `execute_agent_rollback` in `update.rs` |
| `StandaloneBuild` | Build from a GitHub URL (no zip) | `execute_build` in `build/routes.rs` |

Every variant carries a `build_id: Uuid` that is the primary key of the `agent_builds` row.
After execution, the worker reads `agent_builds.status` to determine success or failure — the
executors write their result there rather than returning it.

---

## Lifecycle overview

```
HTTP handler
  │  INSERT INTO build_jobs (payload, status='pending')
  │  build_tx.send(())           ← wake notification (capacity-64 mpsc)
  │  return 202 Accepted
  ▼
build worker (background)
  ├─ drain loop — claims while in-flight < BUILD_CONCURRENCY (default 4)
  │    claim_next_job()          ← SELECT … FOR UPDATE SKIP LOCKED + UPDATE in_progress
  │      │  Ok(None)  → break (nothing claimable)
  │      │  Ok(Some)  → continue
  │    ├─ attempt cap check      ← fail immediately if attempt ≥ MAX_ATTEMPTS (3)
  │    └─ JoinSet::spawn(wrapper)
  │         └─ tokio::task::spawn(execute_claimed_job())
  │              │  Ok(())       → slot frees
  │              └─ is_panic()   → reset_panicked_job() immediately
  │
  └─ select! { join_next | notify | 5s poll | 10min recovery_tick }
```

Jobs run concurrently, but never two for the **same** agent or connector: they would resolve to
the same image tag and deploy the same container, so `claim_next_job` skips a target that already
has a build in flight. The row stays `pending` (it is not failed) and is claimed on a later pass.

---

## Wake mechanism

Three signals wake the drain loop, handled by `tokio::select!`:

1. **mpsc notification** (`build_tx.send(())`) — fired by the HTTP handler that inserts the
   job. Capacity is 64; sends are fire-and-forget (`let _ = build_tx.send(()).await`). A full
   channel means the worker is already awake and draining, which is fine.

2. **5-second fallback poll** — catches any notification that was lost (e.g. channel was full
   at the instant of send). Keeps tail latency bounded without requiring a heartbeat column.

3. **10-minute recovery tick** — calls `recover_stuck_jobs` periodically so jobs stranded by
   a crashed replica are recovered even in a multi-replica cluster where no replica restarts.
   First tick fires 10 minutes after startup (startup already ran recovery inline).

Senders: `upload.rs:365`, `update.rs:287`, `update.rs:697`, `build/routes.rs:211`.

---

## Two-phase execute (panic isolation)

Build executors can panic (e.g. an `unwrap` inside a codec, a bad pointer in native code).
Without isolation, one panicking build would kill the entire worker goroutine and leave all
queued jobs permanently stuck.

The fix separates **claim** (minimal, no panic risk) from **execute** (runs inside
`tokio::task::spawn`):

```rust
// Phase 1 — claim (worker loop, no spawn)
let job = claim_next_job(&state.db).await?;
let job_id   = job.id;
let old_attempt = job.attempt;   // pre-increment; DB now holds old_attempt + 1

// Phase 2 — execute. The outer task holds a concurrency slot; the inner spawn
// is what isolates the panic. Awaiting it here keeps job_id/old_attempt in
// scope, so the reset happens immediately rather than 60 minutes later.
tasks.spawn(async move {
    match tokio::task::spawn(execute_claimed_job(state_clone, job)).await {
        Ok(())                     => { /* done */ }
        Err(ref e) if e.is_panic() => reset_panicked_job(&db, job_id, old_attempt).await,
        Err(_)                     => {}   // unreachable: nothing aborts the inner task
    }
    in_flight.lock().unwrap().remove(&job_id);
});
```

`job_id` and `old_attempt` are captured **before** the spawn, so the panic arm can act on them
without any shared state. The wrapper also clears the job from the in-flight set — see
[Recovery strategies](#recovery-strategies) for why the sweep needs to know what is running.

---

## Attempt counting

The `attempt` counter in `build_jobs` counts how many times a job has been claimed.

- **Increment**: `claim_next_job` increments `attempt` inside the same transaction that marks
  the job `in_progress`. The returned `BuildJob.attempt` is the **pre-increment** value; the
  DB already holds `attempt + 1`.
- **Cap**: `MAX_ATTEMPTS = 3`. Checked immediately after claiming — if `old_attempt >= 3` the
  job is marked `failed` without executing.
- **Panic reset**: `reset_panicked_job` receives `old_attempt`. If `old_attempt + 1 >= 3` it
  permanently fails; otherwise it resets to `pending` for an immediate retry.

---

## Recovery strategies

Two complementary mechanisms handle jobs that got stuck:

### Immediate — `reset_panicked_job`

Called in the `is_panic()` arm of the drain loop. Acts within milliseconds of a panic:

```
old_attempt >= MAX_ATTEMPTS  →  mark 'failed'
otherwise                    →  reset to 'pending' (picked_at = NULL)
```

### Periodic — `recover_stuck_jobs`

Runs at startup and every 10 minutes. Handles jobs left `in_progress` by a crashed replica or
a job that never panicked but also never finished (network partition, OOM, SIGKILL mid-build).
Threshold: `STUCK_JOB_MINS = 60` (2× the max build timeout; avoids false positives on large
images).

```sql
-- Permanently fail exhausted jobs
UPDATE build_jobs SET status = 'failed', error_msg = 'max attempts exceeded', completed_at = now()
WHERE status = 'in_progress'
  AND picked_at < now() - make_interval(mins => 60)
  AND attempt >= 3
  AND id <> ALL($3);   -- jobs this worker is running right now

-- Reset remaining stuck jobs for retry
UPDATE build_jobs SET status = 'pending', picked_at = NULL
WHERE status = 'in_progress'
  AND picked_at < now() - make_interval(mins => 60)
  AND attempt < 3
  AND id <> ALL($3);
```

`make_interval(mins => $2)` keeps the threshold in a single Rust constant rather than duplicated
SQL string literals.

The `id <> ALL($3)` exclusion matters only because the worker is concurrent. While it was serial
the loop sat blocked awaiting the running job, so the recovery tick could never fire mid-build.
A concurrent worker returns to `select!` immediately, so without this a build that legitimately
outlives `STUCK_JOB_MINS` would be reset to `pending`, re-claimed, and run a second time against
the same image tag. Jobs belonging to *other* replicas cannot be vouched for and are still swept.

### Recovery matrix

| Scenario | Recovery | Latency |
|---|---|---|
| Executor panics | `reset_panicked_job` via `is_panic()` arm | Immediate |
| Server crashes mid-build | `recover_stuck_jobs` at next startup | Next restart |
| Multi-replica: peer crashes, no restart | `recover_stuck_jobs` every 10 min | ≤ 70 min |
| Notification lost (channel full) | 5-second fallback poll | ≤ 5 s |

---

## Multi-replica safety

`FOR UPDATE SKIP LOCKED` ensures only one replica claims each job. Two replicas can call
`claim_next_job` simultaneously — the second one skips the locked row and either claims a
different pending job or returns `Ok(None)`.

The same-target rule needs a second mechanism, because `NOT EXISTS` reads committed state: two
replicas could both pass it before either commits, each claiming a sibling of the same target. So
the claim also takes a `pg_try_advisory_xact_lock` keyed on the target id (`'a:'`/`'c:'` prefixed so
the agent and connector id spaces cannot collide). That serializes the read-then-write, and the
loser's subquery then sees the winner's `in_progress` row.

`try_` rather than the blocking form, so a contended target is filtered out and left `pending` for
the next drain instead of holding the transaction open. Note the predicate is evaluated on every
candidate row the scan considers, not only the one `LIMIT` returns, so a busy queue briefly holds a
lock per candidate; they last until the claim transaction commits, but it does mean a concurrent
replica can come up empty while work exists and take it on the next pass.

The `recover_stuck_jobs` queries are safe to run concurrently: both replicas will attempt the
same `UPDATE … WHERE status = 'in_progress' AND picked_at < threshold`. Postgres last-write-wins
per row — the second replica's update is a no-op on rows the first already reset.

---

## Startup and shutdown

**Startup** — `run()` is spawned once in `oss/server/src/state.rs:161`:

```rust
tokio::spawn(crate::agents::build_worker::run(worker_state, build_rx));
```

`recover_stuck_jobs` runs inline before the first `select!` — ensures jobs stranded by the
previous replica are retried before the first HTTP request arrives.

**Shutdown** — when the server receives SIGTERM, Tokio drops the `mpsc::Sender` (`build_tx`).
The `notify.recv()` arm returns `None` and the worker returns **without draining**: in-flight jobs
stay `in_progress` and are re-queued by `recover_stuck_jobs` on the next boot. Waiting instead
could block for a full `build_timeout` (30 min) only to be killed at the pod's termination grace
period. Note that returning drops the `JoinSet`, which aborts each wrapper task; the inner build
task is merely detached (dropping a `JoinHandle` does not cancel it), so a panic during that
window is not converted into an immediate reset — the stuck-job sweep covers it.

---

## Key constants

| Constant | Value | Purpose |
|---|---|---|
| `MAX_ATTEMPTS` | `3` | Maximum claim attempts before permanent failure |
| `STUCK_JOB_MINS` | `60` | Minutes before a job is considered stuck (2× max build timeout) |
| `BUILD_CONCURRENCY` | `4` | Builds in flight at once; clamped to 1..=16. Set `1` for the old serial behavior |
| Channel capacity | `64` | `mpsc::channel(64)` — drops wake if worker is already draining |
| Fallback poll | `5 s` | `tokio::time::sleep(5s)` — catches lost notifications |
| Recovery interval | `10 min` | `interval_at` with `MissedTickBehavior::Skip` |

---

## Adding a new job type

1. Add a variant to `BuildJobPayload` in `upload.rs`.
2. Write an executor function (async, takes `AppState` or individual fields).
3. Add a match arm in `execute_claimed_job` that calls the executor.
4. In the HTTP handler that triggers the build: `INSERT INTO build_jobs` then
   `state.build_tx.send(()).await`.

No changes needed to the worker loop itself.
