//! The harness turns running in this process: what `tasks/cancel` and `tasks/resubscribe` reach
//! them through.
//!
//! A turn's A2A task id is its turn id. While it runs, the turn is registered here under that id
//! with a [`CancellationToken`] (fired by `tasks/cancel`, observed by the turn's driver) and a
//! broadcast of its stream frames (each `message/stream` or `tasks/resubscribe` subscriber gets
//! its own receiver).
//!
//! # Replay
//!
//! A subscriber receives the frames published **from the point it subscribed onward**; a late
//! subscriber is not replayed what it missed. The broadcast keeps the last [`FRAME_BACKLOG`]
//! (256) frames for subscribers that fall behind: one that lags further than that skips the
//! frames it missed and carries on (it never sees a gap marker). The terminal frame is always the
//! last one, and the broadcast closes after it.
//!
//! # Scope
//!
//! The registry is process-wide state, not a field of `AppState`: it holds live handles that
//! mean nothing to another replica. A request that lands on a replica not running the turn is
//! answered from the session row instead (cancel through the row flag), and `tasks/resubscribe`
//! there follows the turn through Redis (below).
//!
//! # Cross-replica frames
//!
//! The pump also publishes every frame to the Redis channel [`frames_channel`]
//! (`nasiko:turn:<turn_id>:frames`, the frame's JSON as the payload) through a [`FramePublisher`]:
//! fire-and-forget, bounded by a small queue, terminal frame last. A publish failure is logged
//! once per turn and never fails the turn; same-replica subscribers never go through Redis.
//!
//! A foreign replica's `tasks/resubscribe` calls [`subscribe_frames`] **before** it sends its Task
//! snapshot, so a frame published between the two is delivered after the snapshot (and the snapshot
//! covers any state it changed); frames published before subscribing are not replayed, the
//! same rule as for a local subscriber. Redis is optional: when it cannot be reached the foreign
//! replica falls back to the snapshot plus polling the row (state changes and the final reply, not
//! every chunk), and the row poll stays as the liveness check for a publisher that died.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use futures::{Stream, StreamExt};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;

/// Frames a lagging subscriber can fall behind by before it starts skipping.
pub const FRAME_BACKLOG: usize = 256;

/// Frames queued for Redis per turn before further non-terminal frames are dropped.
const PUBLISH_BACKLOG: usize = 256;
/// How long to wait for a Redis connection before giving up on it.
const REDIS_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// How long the terminal frame may wait for room in a full publish queue.
const TERMINAL_QUEUE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// A running turn's handles. Cloned freely; dropping a clone does not unregister the turn (that is
/// [`LiveRegistration`]'s job).
#[derive(Clone)]
pub struct LiveTurn {
    pub cancel: CancellationToken,
    frames: broadcast::Sender<String>,
    finished: watch::Sender<bool>,
}

/// A turn's place in the registry. Dropping it removes the entry and tells waiters the turn ended,
/// so the entry cannot outlive whatever owned it: the request being dropped while the turn was
/// still provisioning, a failed start, the driver finishing, or a panic. Held in an `Arc` by
/// everything that must keep the turn registered; the entry goes with the last one, which is the
/// driver, after it has released the session's lease.
pub struct LiveRegistration {
    task_id: String,
    turn: LiveTurn,
}

impl std::ops::Deref for LiveRegistration {
    type Target = LiveTurn;
    fn deref(&self) -> &LiveTurn {
        &self.turn
    }
}

impl Drop for LiveRegistration {
    fn drop(&mut self) {
        registry().remove(&self.task_id);
        // `send_replace` stores the value even when nobody is subscribed yet, so a waiter that
        // subscribes later still sees the turn as ended.
        self.turn.finished.send_replace(true);
    }
}

fn registry() -> MutexGuard<'static, HashMap<String, LiveTurn>> {
    static LIVE: OnceLock<Mutex<HashMap<String, LiveTurn>>> = OnceLock::new();
    LIVE.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The Redis pub/sub channel a turn's frames are published on.
pub fn frames_channel(turn_id: &str) -> String {
    format!("nasiko:turn:{turn_id}:frames")
}

/// Whether a frame ends its turn's stream (`completed`, `failed` or `canceled`).
pub fn is_terminal_frame(frame: &str) -> bool {
    use nasiko_types::a2a::{StreamDisposition, classify_stream_disposition};
    matches!(
        classify_stream_disposition(frame),
        StreamDisposition::Completed | StreamDisposition::Failed
    )
}

/// Publishes a turn's frames to Redis for other replicas, in order, off the turn's path. Cloning
/// shares the queue; the background task ends when the last clone is dropped, after draining.
#[derive(Clone)]
pub struct FramePublisher {
    queue: mpsc::Sender<String>,
}

impl FramePublisher {
    /// Start publishing `turn_id`'s frames through `client`. The connection is made lazily by the
    /// background task, so this never waits on Redis.
    pub fn spawn(client: &redis::Client, turn_id: &str) -> Self {
        let (queue, frames) = mpsc::channel(PUBLISH_BACKLOG);
        tokio::spawn(publish_loop(
            client.clone(),
            frames_channel(turn_id),
            turn_id.to_owned(),
            frames,
        ));
        Self { queue }
    }

    /// Queue one frame. A full queue drops it (the turn never waits on Redis), except a terminal
    /// frame, which waits briefly for room: it is what ends remote streams promptly.
    pub async fn publish(&self, frame: String) {
        if is_terminal_frame(&frame) {
            let _ = self.queue.send_timeout(frame, TERMINAL_QUEUE_WAIT).await;
        } else {
            let _ = self.queue.try_send(frame);
        }
    }
}

async fn publish_loop(
    client: redis::Client,
    channel: String,
    turn_id: String,
    mut frames: mpsc::Receiver<String>,
) {
    use redis::AsyncCommands;
    let mut conn = match tokio::time::timeout(
        REDIS_CONNECT_TIMEOUT,
        client.get_multiplexed_tokio_connection(),
    )
    .await
    {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            tracing::warn!(%turn_id, error = %e, "turn frames are not fanned out: redis unavailable");
            None
        }
        Err(_) => {
            tracing::warn!(%turn_id, "turn frames are not fanned out: redis connect timed out");
            None
        }
    };
    let mut warned = conn.is_none();
    while let Some(frame) = frames.recv().await {
        let Some(conn) = conn.as_mut() else { continue };
        let sent: redis::RedisResult<i64> = conn.publish(&channel, frame).await;
        if let Err(e) = sent
            && !std::mem::replace(&mut warned, true)
        {
            tracing::warn!(%turn_id, error = %e, "publishing a turn frame to redis failed");
        }
    }
}

/// Subscribe to `turn_id`'s frames from other replicas. `None` when Redis cannot be reached (the
/// caller falls back to polling the row). The subscriber is live when this returns, so call it
/// before reading the snapshot it complements.
pub async fn subscribe_frames(
    client: &redis::Client,
    turn_id: &str,
) -> Option<std::pin::Pin<Box<dyn Stream<Item = String> + Send>>> {
    let connect = tokio::time::timeout(REDIS_CONNECT_TIMEOUT, async {
        let mut pubsub = client.get_async_pubsub().await?;
        pubsub.subscribe(frames_channel(turn_id)).await?;
        redis::RedisResult::Ok(pubsub)
    })
    .await;
    match connect {
        Ok(Ok(pubsub)) => {
            Some(Box::pin(pubsub.into_on_message().filter_map(
                |msg| async move { msg.get_payload::<String>().ok() },
            )))
        }
        Ok(Err(e)) => {
            tracing::debug!(%turn_id, error = %e, "could not subscribe to redis frames; polling the row");
            None
        }
        Err(_) => {
            tracing::debug!(%turn_id, "subscribing to redis frames timed out; polling the row");
            None
        }
    }
}

/// Frames as an SSE response, ending when the broadcast closes. A subscriber that fell more than
/// [`FRAME_BACKLOG`] behind skips what it missed. Keep-alive comments are sent so an idle stream
/// (a turn waiting on a human) is not cut by a proxy.
pub fn sse(mut frames: broadcast::Receiver<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    use axum::response::sse::{Event, KeepAlive, Sse};
    let stream = async_stream::stream! {
        loop {
            match frames.recv().await {
                Ok(frame) => yield Ok::<Event, std::convert::Infallible>(Event::default().data(frame)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

impl LiveTurn {
    /// Register a turn under its task id and return its registration with the first subscriber,
    /// taken before anything is published so it misses no frame.
    pub fn register(
        task_id: &str,
    ) -> (
        std::sync::Arc<LiveRegistration>,
        broadcast::Receiver<String>,
    ) {
        let (frames, first) = broadcast::channel(FRAME_BACKLOG);
        let (finished, _) = watch::channel(false);
        let turn = Self {
            cancel: CancellationToken::new(),
            frames,
            finished,
        };
        registry().insert(task_id.to_owned(), turn.clone());
        (
            std::sync::Arc::new(LiveRegistration {
                task_id: task_id.to_owned(),
                turn,
            }),
            first,
        )
    }

    /// The running turn for `task_id`, if this process runs it.
    pub fn get(task_id: &str) -> Option<Self> {
        registry().get(task_id).cloned()
    }

    /// How many turns this process has registered (diagnostics and tests).
    pub fn registered() -> usize {
        registry().len()
    }

    /// A new subscriber: frames published from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.frames.subscribe()
    }

    /// Publish one frame to every subscriber.
    pub fn publish(&self, frame: String) {
        // No subscriber left is fine: the turn does not depend on anyone listening.
        let _ = self.frames.send(frame);
    }

    /// A watch that turns `true` when the turn has ended and been unregistered.
    pub fn finished(&self) -> watch::Receiver<bool> {
        self.finished.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_travel_on_a_channel_named_for_the_turn() {
        assert_eq!(frames_channel("t-1"), "nasiko:turn:t-1:frames");
    }

    #[test]
    fn only_a_finished_status_update_is_a_terminal_frame() {
        use nasiko_types::a2a::{canceled, completed, failed, status_event, to_sse_data, working};
        let frame = |event| to_sse_data(&status_event(event));
        assert!(!is_terminal_frame(&frame(working("t", "c"))));
        assert!(is_terminal_frame(&frame(completed("t", "c"))));
        assert!(is_terminal_frame(&frame(failed("t", "c", "no"))));
        assert!(is_terminal_frame(&frame(canceled("t", "c"))));
        assert!(!is_terminal_frame("not json"));
    }

    #[tokio::test]
    async fn a_subscriber_gets_frames_from_its_subscription_point_onward() {
        let (reg, mut first) = LiveTurn::register("t-live-1");
        reg.publish("a".into());
        let mut late = LiveTurn::get("t-live-1").unwrap().subscribe();
        reg.publish("b".into());
        assert_eq!(first.recv().await.unwrap(), "a");
        assert_eq!(first.recv().await.unwrap(), "b");
        assert_eq!(
            late.recv().await.unwrap(),
            "b",
            "no replay of what it missed"
        );
        drop(reg);
        assert!(LiveTurn::get("t-live-1").is_none());
    }

    #[tokio::test]
    async fn dropping_the_registration_closes_the_stream_and_wakes_waiters() {
        let (reg, mut first) = LiveTurn::register("t-live-2");
        let mut done = reg.finished();
        let handle = reg.turn.clone();
        drop(reg);
        assert!(LiveTurn::get("t-live-2").is_none(), "unregistered on drop");
        assert!(done.changed().await.is_ok());
        assert!(*done.borrow());
        drop(handle);
        assert!(
            first.recv().await.is_err(),
            "closed once the last sender is gone"
        );
    }

    #[tokio::test]
    async fn a_waiter_that_subscribes_after_the_end_still_sees_it() {
        let (reg, _first) = LiveTurn::register("t-live-3");
        let handle = reg.turn.clone();
        drop(reg);
        assert!(*handle.finished().borrow());
    }
}
