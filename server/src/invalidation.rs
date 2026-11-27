//! Cross-replica invalidation: tell every other server replica that something it holds in memory
//! is stale.
//!
//! A replica that changes shared state (an LLM config, a connector's tools, a queued build) drops
//! or rebuilds its own copy at once, as it always has, then calls [`Invalidations::publish`] (or
//! [`publish_key`](Invalidations::publish_key)). Every *other* replica receives it on the Redis
//! pub/sub channel [`CHANNEL`] and runs the handlers it [`register`](Invalidations::register)ed
//! for that topic. The publisher's own handlers are not run: its local invalidation already
//! happened, and some (the tool-search rebuild) are too costly to do twice.
//!
//! # Delivery
//!
//! Best effort, like every pub/sub message. Publishing never makes its caller wait on Redis: it is
//! spawned onto the shared connection. A replica whose listener was down may have missed
//! messages, so each time it resubscribes after the first it runs each handler once with
//! [`Invalidated::All`] (a resync). When Redis cannot be reached at all, local invalidation still
//! happens and each cache's own TTL or poll bounds how stale the others get; the failure is
//! logged at warn at most once a minute.
//!
//! # Handlers
//!
//! Called on the subscriber task, so they must be quick and must not block: drop a cache entry,
//! `try_send` a nudge, or spawn the real work. A topic nobody registered is ignored, so a replica
//! of an older version tolerates messages from a newer one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use futures::StreamExt;

/// The one pub/sub channel every invalidation is published on.
pub const CHANNEL: &str = "nasiko:invalidate";

/// The OSS topics. An edition may define its own (a topic is any `&'static str` without a
/// newline).
pub mod topic {
    /// The MCP tool-search index must be rebuilt from the database.
    pub const MCP_TOOL_SEARCH: &str = "mcp-tool-search";
    /// The resolved-LLM-config cache: all of it, or one agent's entry (key = agent UUID).
    pub const LLM_CONFIG: &str = "llm-config";
    /// A build job was queued: wake the build worker.
    pub const BUILD_QUEUE: &str = "build-queue";
    /// A HITL request was resolved: wake the resume dispatcher.
    pub const HITL_RESUME: &str = "hitl-resume";
}

/// How long one attempt to subscribe may take.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(2);
/// First wait before resubscribing after a failure; doubled up to [`MAX_BACKOFF`].
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// At most one warning of each kind per this interval while Redis stays unavailable.
const WARN_INTERVAL: Duration = Duration::from_secs(60);

/// What a handler is told is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalidated<'a> {
    /// Everything under the topic (also sent on a resync).
    All,
    /// One entry, named by the publisher.
    Key(&'a str),
}

type Handler = Arc<dyn Fn(Invalidated<'_>) + Send + Sync>;

/// Publishes invalidations to other replicas and runs this replica's handlers for theirs. Cheap
/// to clone: clones share the handlers and the subscriber.
#[derive(Clone)]
pub struct Invalidations {
    inner: Arc<Inner>,
}

struct Inner {
    redis: nasiko_redis::SharedRedis,
    /// Identifies this process's messages, so it skips its own.
    origin: String,
    handlers: RwLock<HashMap<&'static str, Vec<Handler>>>,
    publish_warning: WarnThrottle,
}

impl Invalidations {
    /// The bus, with its subscriber task started. Call once per process, inside a Tokio runtime.
    /// The task ends when the last clone is dropped (noticed at its next message or reconnect).
    pub fn start(redis: nasiko_redis::SharedRedis) -> Self {
        let bus = Self::new(redis);
        tokio::spawn(run_subscriber(
            Arc::downgrade(&bus.inner),
            bus.inner.redis.client().clone(),
        ));
        bus
    }

    fn new(redis: nasiko_redis::SharedRedis) -> Self {
        Self {
            inner: Arc::new(Inner {
                redis,
                origin: uuid::Uuid::new_v4().to_string(),
                handlers: RwLock::new(HashMap::new()),
                publish_warning: WarnThrottle::default(),
            }),
        }
    }

    /// Run `handler` whenever another replica publishes `topic`, and on every resync.
    pub fn register(
        &self,
        topic: &'static str,
        handler: impl Fn(Invalidated<'_>) + Send + Sync + 'static,
    ) {
        self.inner
            .handlers
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(topic)
            .or_default()
            .push(Arc::new(handler));
    }

    /// Tell other replicas everything under `topic` is stale. Returns at once.
    pub fn publish(&self, topic: &'static str) {
        self.spawn_publish(topic, String::new());
    }

    /// Tell other replicas one entry under `topic` is stale. Returns at once.
    pub fn publish_key(&self, topic: &'static str, key: impl Into<String>) {
        self.spawn_publish(topic, key.into());
    }

    fn spawn_publish(&self, topic: &'static str, key: String) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let inner = self.inner.clone();
        runtime.spawn(async move {
            if let Err(e) = inner.publish_now(topic, &key).await
                && inner.publish_warning.allow()
            {
                tracing::warn!(
                    topic,
                    error = %e,
                    "could not tell other replicas of an invalidation; their caches catch up by TTL"
                );
            }
        });
    }
}

impl Inner {
    async fn publish_now(&self, topic: &str, key: &str) -> redis::RedisResult<()> {
        use redis::AsyncCommands;
        let mut conn = self.redis.connection().await?;
        let _receivers: i64 = conn
            .publish(CHANNEL, encode(&self.origin, topic, key))
            .await?;
        Ok(())
    }

    fn handlers_for(&self, topic: &str) -> Vec<Handler> {
        self.handlers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(topic)
            .cloned()
            .unwrap_or_default()
    }

    /// Run the handlers a message from another replica names.
    fn dispatch(&self, payload: &str) {
        let Some((origin, topic, key)) = decode(payload) else {
            tracing::debug!(payload, "ignoring a malformed invalidation");
            return;
        };
        if origin == self.origin {
            return;
        }
        let what = if key.is_empty() {
            Invalidated::All
        } else {
            Invalidated::Key(key)
        };
        for handler in self.handlers_for(topic) {
            handler(what);
        }
    }

    /// Run every handler with [`Invalidated::All`]: messages may have been missed.
    fn resync(&self) {
        let handlers: Vec<Handler> = self
            .handlers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .flatten()
            .cloned()
            .collect();
        for handler in handlers {
            handler(Invalidated::All);
        }
    }
}

fn encode(origin: &str, topic: &str, key: &str) -> String {
    format!("{origin}\n{topic}\n{key}")
}

/// `(origin, topic, key)`; the key is empty for "everything".
fn decode(payload: &str) -> Option<(&str, &str, &str)> {
    let mut parts = payload.splitn(3, '\n');
    Some((parts.next()?, parts.next()?, parts.next()?))
}

/// Keeps a listener for as long as the bus lives, resubscribing with backoff whenever it
/// drops. Every resubscribe after the first attempt resyncs, since messages published while it
/// was down are gone.
async fn run_subscriber(bus: Weak<Inner>, client: redis::Client) {
    let warning = WarnThrottle::default();
    let mut backoff = MIN_BACKOFF;
    let mut first_attempt = true;
    loop {
        let subscribed = tokio::time::timeout(SUBSCRIBE_TIMEOUT, async {
            let mut pubsub = client.get_async_pubsub().await?;
            pubsub.subscribe(CHANNEL).await?;
            redis::RedisResult::Ok(pubsub)
        })
        .await;
        match subscribed {
            Ok(Ok(pubsub)) => {
                backoff = MIN_BACKOFF;
                if !std::mem::replace(&mut first_attempt, false) {
                    let Some(inner) = bus.upgrade() else { return };
                    tracing::info!("resubscribed to cross-replica invalidations; resyncing");
                    inner.resync();
                }
                let mut messages = pubsub.into_on_message();
                while let Some(message) = messages.next().await {
                    let Some(inner) = bus.upgrade() else { return };
                    match message.get_payload::<String>() {
                        Ok(payload) => inner.dispatch(&payload),
                        Err(e) => tracing::debug!(error = %e, "ignoring a non-text invalidation"),
                    }
                }
                if warning.allow() {
                    tracing::warn!("lost the cross-replica invalidation listener; resubscribing");
                }
            }
            Ok(Err(e)) => {
                first_attempt = false;
                if warning.allow() {
                    tracing::warn!(error = %e, "cannot subscribe to cross-replica invalidations; caches catch up by TTL");
                }
            }
            Err(_) => {
                first_attempt = false;
                if warning.allow() {
                    tracing::warn!(
                        "subscribing to cross-replica invalidations timed out; caches catch up by TTL"
                    );
                }
            }
        }
        if bus.strong_count() == 0 {
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Lets one warning through per [`WARN_INTERVAL`].
#[derive(Default)]
struct WarnThrottle {
    last: Mutex<Option<Instant>>,
}

impl WarnThrottle {
    fn allow(&self) -> bool {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        if last.is_some_and(|at| now.duration_since(at) < WARN_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn unreachable_redis() -> nasiko_redis::SharedRedis {
        nasiko_redis::SharedRedis::new(redis::Client::open("redis://127.0.0.1:1/").unwrap())
    }

    /// Records what each handler call was told.
    fn recorder(bus: &Invalidations, topic: &'static str) -> Arc<Mutex<Vec<Option<String>>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        bus.register(topic, move |what| {
            sink.lock().unwrap().push(match what {
                Invalidated::All => None,
                Invalidated::Key(k) => Some(k.to_owned()),
            });
        });
        seen
    }

    #[test]
    fn a_message_from_another_replica_runs_that_topics_handlers() {
        let bus = Invalidations::new(unreachable_redis());
        let llm = recorder(&bus, topic::LLM_CONFIG);
        let search = recorder(&bus, topic::MCP_TOOL_SEARCH);

        bus.inner
            .dispatch(&encode("other", topic::LLM_CONFIG, "agent-1"));
        bus.inner.dispatch(&encode("other", topic::LLM_CONFIG, ""));

        assert_eq!(*llm.lock().unwrap(), vec![Some("agent-1".to_owned()), None]);
        assert!(search.lock().unwrap().is_empty());
    }

    #[test]
    fn a_replica_skips_its_own_messages() {
        let bus = Invalidations::new(unreachable_redis());
        let seen = recorder(&bus, topic::LLM_CONFIG);
        let own = bus.inner.origin.clone();
        bus.inner.dispatch(&encode(&own, topic::LLM_CONFIG, ""));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn unknown_topics_and_malformed_payloads_are_ignored() {
        let bus = Invalidations::new(unreachable_redis());
        let seen = recorder(&bus, topic::LLM_CONFIG);
        bus.inner
            .dispatch(&encode("other", "from-a-newer-version", ""));
        bus.inner.dispatch("no separators");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_resync_tells_every_handler_everything_is_stale() {
        let bus = Invalidations::new(unreachable_redis());
        let llm = recorder(&bus, topic::LLM_CONFIG);
        let search = recorder(&bus, topic::MCP_TOOL_SEARCH);
        bus.inner.resync();
        assert_eq!(*llm.lock().unwrap(), vec![None]);
        assert_eq!(*search.lock().unwrap(), vec![None]);
    }

    #[test]
    fn warnings_are_throttled() {
        let throttle = WarnThrottle::default();
        assert!(throttle.allow());
        assert!(!throttle.allow());
    }

    /// With Redis down, publishing neither blocks nor panics.
    #[tokio::test]
    async fn publishing_without_redis_returns_at_once() {
        let bus = Invalidations::start(unreachable_redis());
        let started = Instant::now();
        bus.publish(topic::LLM_CONFIG);
        bus.publish_key(topic::LLM_CONFIG, "agent-1");
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(bus.inner.publish_now(topic::LLM_CONFIG, "").await.is_err());
    }

    /// Two buses on a real Redis stand in for two replicas: one's publish reaches the other's
    /// handler, never its own. Needs `just infra` (REDIS_URL or localhost:6379).
    #[tokio::test]
    #[ignore = "needs Redis"]
    async fn a_publish_reaches_the_other_replica_only() {
        let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
        let redis = || nasiko_redis::SharedRedis::new(redis::Client::open(url.as_str()).unwrap());
        let publisher = Invalidations::start(redis());
        let receiver = Invalidations::start(redis());
        let own = Arc::new(AtomicUsize::new(0));
        let other = Arc::new(AtomicUsize::new(0));
        let (own_count, other_count) = (own.clone(), other.clone());
        publisher.register(topic::BUILD_QUEUE, move |_| {
            own_count.fetch_add(1, Ordering::SeqCst);
        });
        receiver.register(topic::BUILD_QUEUE, move |_| {
            other_count.fetch_add(1, Ordering::SeqCst);
        });

        // Both subscribers are live once the channel has at least two.
        let mut conn = publisher.inner.redis.connection().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (_, subscribers): (String, i64) = redis::cmd("PUBSUB")
                .arg("NUMSUB")
                .arg(CHANNEL)
                .query_async(&mut conn)
                .await
                .unwrap();
            if subscribers >= 2 || Instant::now() > deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        publisher.publish(topic::BUILD_QUEUE);
        let deadline = Instant::now() + Duration::from_secs(5);
        while other.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(other.load(Ordering::SeqCst), 1);
        assert_eq!(own.load(Ordering::SeqCst), 0);
    }
}
