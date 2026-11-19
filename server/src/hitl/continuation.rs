//! Continuation replay buffer for a resumed HITL execution.
//!
//! `deliver()` (`hitl/mod.rs`) is the sole client of the agent's resume call — decoupled from any
//! browser connection by design (§3.2), so its retry/crash-recovery semantics survive a closed
//! tab. That decoupling is exactly why the resumed agent's real A2A/SSE events never reach the
//! frontend today: nothing captures them.
//!
//! This registry closes that gap without coupling the two together. `deliver()` (and its
//! orchestrator-turn helper) append the *actual* bytes they already read from the agent into a
//! small buffer, keyed by the `hitl_requests.id` that was just resolved — the same id the frontend
//! already holds, since it's the one it just POSTed to `/resolve`. A reconnecting request (routed
//! through the existing `POST /api/orchestrator/a2a`, gated on `metadata.reconnect_after_hitl_id`
//! — see `a2a_dispatch.rs::reconnect_stream`) replays whatever is already buffered, then
//! live-tails anything appended after that, closing once the buffer is marked terminal. Because
//! replay reads from a durable-for-the-duration-of-the-execution buffer rather than subscribing to
//! a transient broadcast, a reconnect that arrives after the resume already finished still gets
//! the complete sequence — there is no "arrived too late" case.
//!
//! # Across replicas
//!
//! The resume runs on whichever replica claimed the row; the browser's reconnect lands on any
//! replica. So the buffer is a Redis Stream per id ([`stream_key`]), not process memory:
//!
//! - Each entry has a kind field: an event (`ev`, with the SSE data), the terminal marker
//!   (`end`), or the marker that creates the stream (`open`, skipped by readers).
//! - The stream is created only by [`OPEN_SCRIPT`], with a [`STALE_NON_TERMINAL_TTL`] expiry, by
//!   whichever comes first: `deliver()`'s guard, an alias, or a `watch`. Appends use `NOMKSTREAM`,
//!   so no key ever exists without an expiry. The terminal marker shortens it to [`BUFFER_TTL`].
//!   Expiry replaces the in-memory sweep.
//! - An alias is a key ([`alias_key`]) holding the target id, with the stale expiry.
//! - `watch` replays from the start with `XREAD`, then tails with `XREAD BLOCK` on a connection
//!   of its own (a blocking read holds its connection; see `nasiko_redis`).
//! - `deliver()`'s appends go through a queue to one writer task per resume, in order, so the
//!   resume never waits on Redis.

use std::sync::Mutex;
use std::time::Duration;

use futures::Stream;
use nasiko_redis::SharedRedis;
use redis::AsyncCommands;
use redis::streams::{StreamReadOptions, StreamReadReply};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Maximum events retained per buffer. In practice a HITL resume produces at most a handful of
/// A2A events before completing or pausing again — but that premise doesn't hold for two paths
/// that append into this same buffer: `trigger_new_orchestrator_turn` (`hitl/mod.rs`) appends
/// every `data:` line of a whole ReAct turn, and `consume_sse_to_terminal` appends every streamed
/// `artifactUpdate` chunk. Either can be arbitrarily long for a chatty or looping agent, and
/// `BUFFER_TTL` keeps a completed buffer resident for 10 minutes — so resident memory without a
/// per-buffer cap is (resumes in any 10-minute window) x (each one's entire streamed output),
/// reachable by a single user. Once hit, further events are dropped and a single truncation
/// marker (see `TRUNCATION_MARKER`) is appended instead, so a reconnecting client sees an explicit
/// "replay was partial" signal rather than a silently incomplete transcript.
const MAX_BUFFERED_EVENTS: usize = 500;
/// Byte-size cap alongside the event-count cap — a handful of very large events (e.g. big artifact
/// chunks) could otherwise exceed a reasonable memory bound while staying under
/// `MAX_BUFFERED_EVENTS`.
const MAX_BUFFERED_BYTES: usize = 1 << 20; // 1 MiB
/// Backstop length for the stream (`XADD MAXLEN ~`). The per-resume cap above keeps a stream far
/// shorter; this only bounds one written to by several delivery attempts.
const STREAM_MAX_LEN: usize = 2 * MAX_BUFFERED_EVENTS;

/// Appended once, in place of any further events, when a buffer hits `MAX_BUFFERED_EVENTS`/
/// `MAX_BUFFERED_BYTES` — a normal `working`-status SSE payload (the same wire shape every other
/// buffered event uses) whose text a client's existing status-line rendering already knows how to
/// show, so no new client-side parsing is required to surface the truncation.
const TRUNCATION_MARKER: &str = r#"{"result":{"statusUpdate":{"status":{"state":"working","message":{"parts":[{"text":"[replay truncated: this resume produced more events than can be buffered; some output was dropped]"}]}}}}}"#;

/// How long a finished buffer is kept around for a late reconnect before it expires.
/// Generous on purpose: a human reading a final response and only then closing/reloading the tab
/// should still find it there a few minutes later.
const BUFFER_TTL: Duration = Duration::from_secs(10 * 60);
/// Backstop only — no real `deliver()` call should ever take this long. Long enough that it never
/// fires on a genuinely slow-but-live delivery, short enough that a bug reintroducing an
/// un-terminated buffer still gets cleaned up rather than living forever.
const STALE_NON_TERMINAL_TTL: Duration = Duration::from_secs(60 * 60);
/// How long one `XREAD BLOCK` in `watch()` waits before re-checking whether its buffer has expired
/// out from under it. Short enough that a stream whose buffer expired unwedges soon after, long
/// enough that it never fires spuriously on a genuinely slow-but-live delivery.
const WATCH_NOTIFY_TIMEOUT: Duration = Duration::from_secs(90);
/// Entries `watch()` reads per round trip.
const READ_BATCH: usize = 100;

/// Stream entry field naming the entry's kind, and the kinds a reader acts on (it skips any other,
/// such as [`OPEN_SCRIPT`]'s `open`).
const KIND_FIELD: &str = "k";
const DATA_FIELD: &str = "d";
const KIND_EVENT: &str = "ev";
const KIND_TERMINAL: &str = "end";

/// Creates `KEYS[1]` with an `open` entry and an expiry of `ARGV[1]` seconds, unless it exists.
/// Atomic, so a stream never exists without an expiry and an existing one's expiry (shortened
/// once terminal) is never reset.
const OPEN_SCRIPT: &str = r"
if redis.call('EXISTS', KEYS[1]) == 0 then
    redis.call('XADD', KEYS[1], '*', 'k', 'open')
    redis.call('EXPIRE', KEYS[1], ARGV[1])
end
return 1
";

/// The Redis Stream holding `id`'s buffer.
fn stream_key(id: Uuid) -> String {
    format!("nasiko:hitl:continuation:{id}")
}

/// The key that makes `alias` resolve to another id's buffer.
fn alias_key(alias: Uuid) -> String {
    format!("nasiko:hitl:continuation:{alias}:alias")
}

/// Every resolved `hitl_requests.id`'s continuation buffer, in Redis (see the module docs).
#[derive(Clone)]
pub struct ContinuationRegistry {
    redis: SharedRedis,
}

impl ContinuationRegistry {
    pub fn new(redis: SharedRedis) -> Self {
        Self { redis }
    }

    /// The id whose buffer `id` reads and writes: the target of an alias, else `id` itself.
    async fn resolve(&self, id: Uuid) -> redis::RedisResult<Uuid> {
        let mut conn = self.redis.connection().await?;
        let target: Option<String> = conn.get(alias_key(id)).await?;
        Ok(target.and_then(|t| t.parse().ok()).unwrap_or(id))
    }

    /// Create `id`'s buffer unless it exists (see [`OPEN_SCRIPT`]).
    async fn open(&self, id: Uuid) -> redis::RedisResult<()> {
        let mut conn = self.redis.connection().await?;
        redis::Script::new(OPEN_SCRIPT)
            .key(stream_key(id))
            .arg(STALE_NON_TERMINAL_TTL.as_secs())
            .invoke_async(&mut conn)
            .await
    }

    /// Whether a buffer for `id` already exists — i.e. whether *something* (a `ContinuationGuard`
    /// or an `alias`) has actually claimed this id for delivery. Unlike `watch`, this never creates
    /// one: it's the caller's way to tell "in flight or already finished" (a buffer exists) apart
    /// from "nothing has ever been dispatched for this id, and nothing ever will be until it is"
    /// (no buffer, and `watch` would otherwise conjure one up that nothing ever terminates).
    /// Redis being unreachable reads as no buffer.
    pub async fn exists(&self, id: Uuid) -> bool {
        let found = async {
            let key = stream_key(self.resolve(id).await?);
            let mut conn = self.redis.connection().await?;
            conn.exists::<_, bool>(key).await
        };
        found.await.unwrap_or_else(|e| {
            tracing::warn!(%id, error = %e, "could not check for a hitl continuation in redis");
            false
        })
    }

    /// Makes `alias` resolve to the exact same buffer as `target`. Needed for the MCP-mirror pause
    /// case: `resolve_display_row` (`oss/hitl/src/store.rs`) shows the frontend the *real*
    /// `mcp_tool` row's id, so that's the id a reconnect arrives with — but `deliver()` only ever
    /// runs (and keys its `ContinuationGuard`) on the *mirror* row's id, the one
    /// `auto_resolve_linked_direct_chat_row` (`oss/server/src/router/hitl.rs`) actually resolves to
    /// trigger the resume. Without this, a reconnect for the real row's id watches a buffer nothing
    /// ever writes to or terminates. Call site sets this up before the dispatcher can possibly
    /// finish delivering, so a reconnect racing ahead of `deliver()` still lands on the shared
    /// buffer either way — opening the target here is what makes the two insertion orders
    /// equivalent. Best effort: a Redis failure is logged.
    pub async fn alias(&self, alias: Uuid, target: Uuid) {
        let linked = async {
            let target = self.resolve(target).await?;
            self.open(target).await?;
            let mut conn = self.redis.connection().await?;
            conn.set_ex::<_, _, ()>(
                alias_key(alias),
                target.to_string(),
                STALE_NON_TERMINAL_TTL.as_secs(),
            )
            .await
        };
        if let Err(e) = linked.await {
            tracing::warn!(%alias, %target, error = %e, "could not alias a hitl continuation in redis");
        }
    }

    /// Replay-then-live-tail `id`'s buffer as a plain async generator of already-normalized SSE
    /// data strings. The caller (`a2a_dispatch.rs::reconnect_stream`) wraps each yielded string in
    /// an `Event::default().data(...)`, identical to how a live turn's own generator does — this
    /// function only knows about the buffer, not the HTTP/SSE framing around it. Ends once every
    /// entry is read and the terminal marker was among them, when the buffer has expired, or when
    /// Redis fails.
    pub fn watch(&self, id: Uuid) -> impl Stream<Item = String> + 'static {
        let registry = self.clone();
        async_stream::stream! {
            let opened = async {
                let id = registry.resolve(id).await?;
                registry.open(id).await?;
                let reader = registry.redis.client().get_multiplexed_async_connection().await?;
                redis::RedisResult::Ok((stream_key(id), reader))
            };
            let (key, mut reader) = match opened.await {
                Ok(opened) => opened,
                Err(e) => {
                    tracing::warn!(%id, error = %e, "could not watch a hitl continuation in redis");
                    return;
                }
            };
            let mut cursor = "0-0".to_string();
            let mut terminal = false;
            loop {
                // Once the terminal marker is read, only drain what is left: never block.
                let wait = (!terminal).then_some(WATCH_NOTIFY_TIMEOUT);
                let entries = match read_after(&mut reader, &key, &cursor, wait).await {
                    Ok(entries) => entries,
                    Err(e) => {
                        tracing::warn!(%id, error = %e, "reading a hitl continuation from redis failed");
                        break;
                    }
                };
                if entries.is_empty() {
                    if terminal || !registry.stream_exists(&key).await {
                        break;
                    }
                    continue;
                }
                for entry in entries {
                    cursor = entry.id.clone();
                    match entry_kind(&entry).as_deref() {
                        Some(KIND_EVENT) => {
                            if let Some(data) = entry.get::<String>(DATA_FIELD) {
                                yield data;
                            }
                        }
                        Some(KIND_TERMINAL) => terminal = true,
                        _ => {}
                    }
                }
            }
        }
    }

    /// Whether `key` still exists. A failed check reads as gone, which ends a watch rather than
    /// leaving it to block on a Redis it cannot reach.
    async fn stream_exists(&self, key: &str) -> bool {
        match self.redis.connection().await {
            Ok(mut conn) => conn.exists(key).await.unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// Entries of `key` after `cursor`, waiting up to `wait` for one when there are none yet.
async fn read_after(
    reader: &mut redis::aio::MultiplexedConnection,
    key: &str,
    cursor: &str,
    wait: Option<Duration>,
) -> redis::RedisResult<Vec<redis::streams::StreamId>> {
    let mut options = StreamReadOptions::default().count(READ_BATCH);
    if let Some(wait) = wait {
        options = options.block(wait.as_millis() as usize);
    }
    let reply: Option<StreamReadReply> = reader.xread_options(&[key], &[cursor], &options).await?;
    Ok(reply
        .into_iter()
        .flat_map(|reply| reply.keys)
        .flat_map(|stream| stream.ids)
        .collect())
}

fn entry_kind(entry: &redis::streams::StreamId) -> Option<String> {
    entry.get(KIND_FIELD)
}

/// What one resume may still append: `MAX_BUFFERED_EVENTS` events and `MAX_BUFFERED_BYTES`, then
/// one `TRUNCATION_MARKER`, then nothing.
#[derive(Default)]
struct AppendBudget {
    events: usize,
    bytes: usize,
    truncated: bool,
}

impl AppendBudget {
    /// What to append for `data`: `data` itself, the truncation marker the first time the cap is
    /// hit (or would be by this event), or nothing once truncated.
    fn admit(&mut self, data: String) -> Option<String> {
        if self.truncated {
            return None;
        }
        if self.events >= MAX_BUFFERED_EVENTS || self.bytes + data.len() > MAX_BUFFERED_BYTES {
            self.truncated = true;
            return Some(TRUNCATION_MARKER.to_string());
        }
        self.events += 1;
        self.bytes += data.len();
        Some(data)
    }
}

/// RAII guard tying a continuation buffer's lifetime to `deliver()`'s own call frame. `deliver()`
/// has many early-return paths (agent lookup failure, flow-guard rejection, HTTP error, ...) —
/// rather than adding a terminal write to each one, dropping this guard closes its writer's queue,
/// and the writer appends the terminal marker after the last queued event, once `deliver()`
/// returns by any path, including an unwind. `push` is the only other operation exposed —
/// appending is the one thing `deliver()`'s consumption helpers need mid-flight.
pub struct ContinuationGuard {
    queue: mpsc::UnboundedSender<String>,
    budget: Mutex<AppendBudget>,
}

impl ContinuationGuard {
    /// Open `id`'s buffer and start its writer. A Redis failure is logged; the resume goes on
    /// without a buffer.
    pub async fn open(registry: &ContinuationRegistry, id: Uuid) -> Self {
        let id = match registry.resolve(id).await {
            Ok(target) => target,
            Err(_) => id,
        };
        if let Err(e) = registry.open(id).await {
            tracing::warn!(%id, error = %e, "could not open a hitl continuation in redis");
        }
        let (queue, events) = mpsc::unbounded_channel();
        tokio::spawn(write_loop(registry.redis.clone(), id, events));
        Self {
            queue,
            budget: Mutex::new(AppendBudget::default()),
        }
    }

    /// Append one real, already-normalized SSE `data:` payload. Non-blocking (a queue send, no
    /// I/O) by design — this must never be able to affect `deliver()`'s own retry/completion
    /// logic. The budget bounds what is queued.
    pub fn push(&self, data: String) {
        let admitted = self
            .budget
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admit(data);
        if let Some(data) = admitted {
            let _ = self.queue.send(data);
        }
    }
}

/// Appends one resume's events to its stream in order, then the terminal marker once the guard is
/// dropped. After a failed append the remaining events are skipped (best effort; the resume itself
/// is unaffected), but the terminal marker is still attempted so a watcher ends.
async fn write_loop(redis: SharedRedis, id: Uuid, mut events: mpsc::UnboundedReceiver<String>) {
    let key = stream_key(id);
    let mut failed = false;
    while let Some(data) = events.recv().await {
        if failed {
            continue;
        }
        if let Err(e) = append(
            &redis,
            &key,
            &[(KIND_FIELD, KIND_EVENT), (DATA_FIELD, &data)],
        )
        .await
        {
            tracing::warn!(%id, error = %e, "appending to a hitl continuation in redis failed");
            failed = true;
        }
    }
    if let Err(e) = append_terminal(&redis, &key).await {
        tracing::warn!(%id, error = %e, "marking a hitl continuation terminal in redis failed");
    }
}

/// `XADD key NOMKSTREAM MAXLEN ~ STREAM_MAX_LEN * fields`: never creates the stream, so an
/// expired buffer stays gone instead of coming back without an expiry.
fn xadd(key: &str, fields: &[(&str, &str)]) -> redis::Cmd {
    let mut cmd = redis::cmd("XADD");
    cmd.arg(key)
        .arg("NOMKSTREAM")
        .arg("MAXLEN")
        .arg("~")
        .arg(STREAM_MAX_LEN)
        .arg("*");
    for (field, value) in fields {
        cmd.arg(*field).arg(*value);
    }
    cmd
}

async fn append(redis: &SharedRedis, key: &str, fields: &[(&str, &str)]) -> redis::RedisResult<()> {
    let mut conn = redis.connection().await?;
    xadd(key, fields).query_async(&mut conn).await
}

/// The terminal marker and the shorter expiry, together.
async fn append_terminal(redis: &SharedRedis, key: &str) -> redis::RedisResult<()> {
    let mut conn = redis.connection().await?;
    redis::pipe()
        .atomic()
        .add_command(xadd(key, &[(KIND_FIELD, KIND_TERMINAL)]))
        .ignore()
        .expire(key, BUFFER_TTL.as_secs() as i64)
        .ignore()
        .query_async(&mut conn)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admitted(budget: &mut AppendBudget, data: String) -> Vec<String> {
        budget.admit(data).into_iter().collect()
    }

    #[test]
    fn append_past_the_event_cap_truncates_with_a_marker_and_stops_growing() {
        let mut budget = AppendBudget::default();
        let events: Vec<String> = (0..MAX_BUFFERED_EVENTS + 10)
            .flat_map(|i| admitted(&mut budget, format!("event-{i}")))
            .collect();
        assert_eq!(
            events.len(),
            MAX_BUFFERED_EVENTS + 1,
            "the cap's worth of real events plus exactly one truncation marker, never more"
        );
        assert_eq!(events.last().unwrap(), TRUNCATION_MARKER);
        assert_eq!(
            events[MAX_BUFFERED_EVENTS - 1],
            format!("event-{}", MAX_BUFFERED_EVENTS - 1)
        );
    }

    #[test]
    fn append_past_the_byte_cap_truncates_even_under_the_event_count_cap() {
        let mut budget = AppendBudget::default();
        let big = "x".repeat(MAX_BUFFERED_BYTES / 2 + 1);
        let mut events = admitted(&mut budget, big.clone());
        events.extend(admitted(&mut budget, big));
        events.extend(admitted(&mut budget, "should be dropped".to_string()));
        assert_eq!(
            events.len(),
            2,
            "one real event plus one truncation marker, well under the event-count cap"
        );
        assert_eq!(events[1], TRUNCATION_MARKER);
    }

    #[test]
    fn truncation_is_a_permanent_state_not_just_a_one_time_threshold_crossing() {
        let mut budget = AppendBudget::default();
        for i in 0..MAX_BUFFERED_EVENTS + 1 {
            budget.admit(format!("event-{i}"));
        }
        for i in 0..50 {
            assert!(
                budget.admit(format!("late-event-{i}")).is_none(),
                "once truncated, further appends must be silent no-ops, not grow the buffer again"
            );
        }
    }

    #[test]
    fn appends_never_create_the_stream_and_are_capped() {
        let cmd = xadd("k", &[(KIND_FIELD, KIND_EVENT), (DATA_FIELD, "x")]);
        let args: Vec<String> = cmd
            .args_iter()
            .filter_map(|arg| match arg {
                redis::Arg::Simple(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
                redis::Arg::Cursor => None,
            })
            .collect();
        assert_eq!(
            args,
            [
                "XADD",
                "k",
                "NOMKSTREAM",
                "MAXLEN",
                "~",
                &STREAM_MAX_LEN.to_string(),
                "*",
                "k",
                "ev",
                "d",
                "x"
            ]
        );
    }
}
