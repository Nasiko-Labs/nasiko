//! Live flow events: what a chat stream shows while its turn runs (a tool waiting for approval,
//! an agent waking).
//!
//! A subscriber is the user's chat SSE, on whichever replica took the chat request. A publisher
//! is whichever replica serves the request that caused the event: an agent's own `/api/mcp` call,
//! a proxy hop that wakes a parked agent. With one replica they are the same process; with more
//! they usually are not.
//!
//! # Cross-replica delivery
//!
//! [`FlowEventBus::with_redis`] fans events out over Redis pub/sub, on one channel per flow
//! ([`flow_channel`], `nasiko:flow:<flow_id>:events`):
//!
//! - **Publish** delivers to this process's subscribers directly, then queues the event for
//!   Redis. One background task per bus sends the queue in order on the shared connection; a
//!   full queue drops the event, so a publisher never waits on Redis.
//! - **Receive**: one pub/sub connection per bus, from [`SharedRedis::client`] (pub/sub holds its
//!   connection), `PSUBSCRIBE`d to every flow's channel. It hands each event to this process's
//!   subscribers of that flow and ignores the ones this bus published itself, which it already
//!   delivered. It is opened by the first `subscribe` and reconnects with a backoff after a drop.
//!
//! One pattern listener (`PSUBSCRIBE`) per process rather than a `SUBSCRIBE` per flow: flow events
//! are rare (a few per turn at most), so receiving every flow's and dropping the ones with no local
//! subscriber costs nothing, while a `SUBSCRIBE` per flow would put a Redis round trip on every
//! chat request and race a flow's subscribe against its `remove`. One connection per subscriber
//! would cost a connection per open chat stream.
//!
//! `subscribe` returns once the pattern listener is live, waiting at most
//! [`REDIS_CONNECT_TIMEOUT`] for the first connection, so an event another replica publishes after
//! the chat stream subscribed is not missed. While Redis is down subscribers still get this
//! process's own events. Delivery is best effort either way: the event's durable record (a
//! `hitl_requests` row, the agent's status) is elsewhere.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::StreamExt;
use nasiko_redis::SharedRedis;
use tokio::sync::{OnceCell, RwLock, broadcast, mpsc, watch};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlowEvent {
    AgentInvoke {
        caller_agent: String,
        target_agent: String,
        depth: u32,
    },
    AgentResult {
        caller_agent: String,
        target_agent: String,
        depth: u32,
        success: bool,
        latency_ms: u64,
    },
    /// An MCP tool call needs human approval (ask-stance permission, -32001)
    /// before it can proceed. Surfaced to the chat UI as an approval prompt.
    ToolApprovalRequired {
        agent_id: String,
        server: String,
        tool: String,
    },
    /// A parked agent is being woken to serve this flow's request. `wake_ms` is how long the wake
    /// took when it is announced once done, and `None` when it is announced as it begins.
    AgentWaking {
        agent_id: String,
        wake_ms: Option<u64>,
    },
}

/// Events a slow local subscriber can fall behind by before it skips.
const SUBSCRIBER_BACKLOG: usize = 64;
/// Events queued for Redis before further ones are dropped.
const PUBLISH_BACKLOG: usize = 256;
/// How long `subscribe` waits for the first Redis connection before going on without it.
const REDIS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the receiver waits before reconnecting, doubling up to [`MAX_RECONNECT_WAIT`].
const FIRST_RECONNECT_WAIT: Duration = Duration::from_secs(1);
const MAX_RECONNECT_WAIT: Duration = Duration::from_secs(30);

const CHANNEL_PREFIX: &str = "nasiko:flow:";
const CHANNEL_SUFFIX: &str = ":events";

/// The Redis pub/sub channel a flow's events are published on.
pub fn flow_channel(flow_id: &str) -> String {
    format!("{CHANNEL_PREFIX}{flow_id}{CHANNEL_SUFFIX}")
}

/// The flow a channel named by [`flow_channel`] belongs to.
fn flow_of_channel(channel: &str) -> Option<&str> {
    channel
        .strip_prefix(CHANNEL_PREFIX)?
        .strip_suffix(CHANNEL_SUFFIX)
}

/// An event as sent over Redis: the event and the bus that published it.
#[derive(Serialize, Deserialize)]
struct Envelope {
    origin: String,
    event: FlowEvent,
}

type LocalChannels = RwLock<HashMap<String, broadcast::Sender<FlowEvent>>>;

/// Broadcast bus for flow events, keyed by flow_id. See the module docs for how it crosses
/// replicas.
#[derive(Clone)]
pub struct FlowEventBus {
    channels: Arc<LocalChannels>,
    fanout: Option<Arc<RedisFanout>>,
}

impl Default for FlowEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl FlowEventBus {
    /// A bus that reaches this process's subscribers only.
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            fanout: None,
        }
    }

    /// A bus that also reaches subscribers on other replicas, through Redis. Connects lazily:
    /// nothing is opened until the first publish or subscribe.
    pub fn with_redis(redis: SharedRedis) -> Self {
        Self {
            fanout: Some(Arc::new(RedisFanout::new(redis))),
            ..Self::new()
        }
    }

    pub async fn subscribe(&self, flow_id: &str) -> broadcast::Receiver<FlowEvent> {
        let receiver = {
            let mut channels = self.channels.write().await;
            let tx = channels
                .entry(flow_id.to_string())
                .or_insert_with(|| broadcast::channel(SUBSCRIBER_BACKLOG).0);
            tx.subscribe()
        };
        if let Some(fanout) = &self.fanout {
            fanout.wait_for_receiver(&self.channels).await;
        }
        receiver
    }

    pub async fn publish(&self, flow_id: &str, event: FlowEvent) {
        if let Some(fanout) = &self.fanout {
            fanout.send(flow_id, &event);
        }
        deliver_locally(&self.channels, flow_id, event).await;
    }

    pub async fn remove(&self, flow_id: &str) {
        let mut channels = self.channels.write().await;
        channels.remove(flow_id);
    }
}

async fn deliver_locally(channels: &LocalChannels, flow_id: &str, event: FlowEvent) {
    let channels = channels.read().await;
    if let Some(tx) = channels.get(flow_id) {
        let _ = tx.send(event);
    }
}

/// Whether the receiver's pattern listener is live.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiverState {
    /// The first connection is being opened.
    Connecting,
    Live,
    /// Disconnected; reconnecting in the background.
    Down,
}

/// The Redis side of a bus: an ordered outbound queue and one inbound pattern listener.
struct RedisFanout {
    redis: SharedRedis,
    /// Identifies this bus's own events when they come back from Redis.
    origin: String,
    outbox: std::sync::OnceLock<mpsc::Sender<(String, String)>>,
    receiver: OnceCell<watch::Receiver<ReceiverState>>,
}

impl RedisFanout {
    fn new(redis: SharedRedis) -> Self {
        Self {
            redis,
            origin: uuid::Uuid::new_v4().to_string(),
            outbox: std::sync::OnceLock::new(),
            receiver: OnceCell::new(),
        }
    }

    /// Queue `event` for other replicas. Never waits: a full queue drops it.
    fn send(&self, flow_id: &str, event: &FlowEvent) {
        let envelope = Envelope {
            origin: self.origin.clone(),
            event: event.clone(),
        };
        let Ok(payload) = serde_json::to_string(&envelope) else {
            return;
        };
        let outbox = self.outbox.get_or_init(|| {
            let (queue, events) = mpsc::channel(PUBLISH_BACKLOG);
            tokio::spawn(publish_loop(self.redis.clone(), events));
            queue
        });
        if outbox.try_send((flow_channel(flow_id), payload)).is_err() {
            tracing::debug!(%flow_id, "flow event not fanned out: publish queue full");
        }
    }

    /// Start the receiver if it is not running, and wait (bounded) for its first connection.
    async fn wait_for_receiver(&self, channels: &Arc<LocalChannels>) {
        let mut state = self
            .receiver
            .get_or_init(|| async {
                let (report, state) = watch::channel(ReceiverState::Connecting);
                tokio::spawn(receive_loop(
                    self.redis.client().clone(),
                    self.origin.clone(),
                    Arc::downgrade(channels),
                    report,
                ));
                state
            })
            .await
            .clone();
        let settled = state.wait_for(|s| *s != ReceiverState::Connecting);
        let _ = tokio::time::timeout(REDIS_CONNECT_TIMEOUT, settled).await;
    }
}

/// Sends queued events to Redis in order until the bus is dropped. A failure is logged once and
/// the event is lost.
async fn publish_loop(redis: SharedRedis, mut events: mpsc::Receiver<(String, String)>) {
    let mut warned = false;
    while let Some((channel, payload)) = events.recv().await {
        let sent: redis::RedisResult<i64> = match redis.connection().await {
            Ok(mut conn) => {
                redis::cmd("PUBLISH")
                    .arg(&channel)
                    .arg(payload)
                    .query_async(&mut conn)
                    .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = sent
            && !std::mem::replace(&mut warned, true)
        {
            tracing::warn!(error = %e, "publishing a flow event to redis failed");
        }
    }
}

/// Feeds this process's subscribers from Redis for as long as the bus lives, reconnecting after a
/// drop. Events published while it is disconnected are lost.
async fn receive_loop(
    client: redis::Client,
    origin: String,
    channels: Weak<LocalChannels>,
    report: watch::Sender<ReceiverState>,
) {
    let pattern = flow_channel("*");
    let mut wait = FIRST_RECONNECT_WAIT;
    loop {
        match subscribe_pattern(&client, &pattern).await {
            Ok(pubsub) => {
                report.send_replace(ReceiverState::Live);
                wait = FIRST_RECONNECT_WAIT;
                let mut messages = pubsub.into_on_message();
                while let Some(msg) = messages.next().await {
                    let Some(channels) = channels.upgrade() else {
                        return;
                    };
                    let Ok(payload) = msg.get_payload::<String>() else {
                        continue;
                    };
                    relay(&channels, &origin, msg.get_channel_name(), &payload).await;
                }
                tracing::warn!("flow event listener closed; reconnecting");
            }
            Err(e) => tracing::warn!(error = %e, "could not subscribe to flow events in redis"),
        }
        report.send_replace(ReceiverState::Down);
        if channels.strong_count() == 0 {
            return;
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MAX_RECONNECT_WAIT);
    }
}

async fn subscribe_pattern(
    client: &redis::Client,
    pattern: &str,
) -> redis::RedisResult<redis::aio::PubSub> {
    let open = async {
        let mut pubsub = client.get_async_pubsub().await?;
        pubsub.psubscribe(pattern).await?;
        Ok(pubsub)
    };
    match tokio::time::timeout(REDIS_CONNECT_TIMEOUT, open).await {
        Ok(result) => result,
        Err(_) => Err(redis::RedisError::from((
            redis::ErrorKind::IoError,
            "redis connect timed out",
        ))),
    }
}

/// Hands one event received from Redis to this process's subscribers of its flow, unless this
/// bus published it.
async fn relay(channels: &LocalChannels, origin: &str, channel: &str, payload: &str) {
    let Some(flow_id) = flow_of_channel(channel) else {
        return;
    };
    let Ok(envelope) = serde_json::from_str::<Envelope>(payload) else {
        return;
    };
    if envelope.origin != origin {
        deliver_locally(channels, flow_id, envelope.event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waking(agent_id: &str) -> FlowEvent {
        FlowEvent::AgentWaking {
            agent_id: agent_id.into(),
            wake_ms: None,
        }
    }

    #[test]
    fn a_flow_has_its_own_channel() {
        assert_eq!(flow_channel("f-1"), "nasiko:flow:f-1:events");
        assert_eq!(flow_of_channel("nasiko:flow:f-1:events"), Some("f-1"));
        assert_eq!(flow_of_channel("nasiko:turn:t:frames"), None);
    }

    #[tokio::test]
    async fn an_event_from_another_bus_reaches_local_subscribers() {
        let bus = FlowEventBus::new();
        let mut events = bus.subscribe("f-1").await;
        let payload = serde_json::to_string(&Envelope {
            origin: "other".into(),
            event: waking("a"),
        })
        .unwrap();

        relay(&bus.channels, "mine", &flow_channel("f-1"), &payload).await;

        assert!(
            matches!(events.try_recv(), Ok(FlowEvent::AgentWaking { agent_id, .. }) if agent_id == "a")
        );
    }

    #[tokio::test]
    async fn a_bus_ignores_its_own_events_coming_back_from_redis() {
        let bus = FlowEventBus::new();
        let mut events = bus.subscribe("f-1").await;
        let payload = serde_json::to_string(&Envelope {
            origin: "mine".into(),
            event: waking("a"),
        })
        .unwrap();

        relay(&bus.channels, "mine", &flow_channel("f-1"), &payload).await;

        assert!(events.try_recv().is_err(), "already delivered locally");
    }

    /// Nothing listens on port 1: subscribing does not hang on Redis, and this process's own
    /// events still arrive.
    #[tokio::test]
    async fn local_delivery_does_not_depend_on_redis() {
        let redis = SharedRedis::new(redis::Client::open("redis://127.0.0.1:1/").unwrap());
        let bus = FlowEventBus::with_redis(redis);
        let started = std::time::Instant::now();
        let mut events = bus.subscribe("f-1").await;
        assert!(started.elapsed() <= REDIS_CONNECT_TIMEOUT + Duration::from_millis(500));

        bus.publish("f-1", waking("a")).await;

        assert!(matches!(
            events.try_recv(),
            Ok(FlowEvent::AgentWaking { .. })
        ));
    }
}
