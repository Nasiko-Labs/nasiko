//! One Redis connection shared by every caller in the process.
//!
//! `redis::Client::get_multiplexed_async_connection()` opens a new TCP connection each time it is
//! called. Called per operation, as the request path did, a turn opened several, and each closed
//! connection holds its local port for about 60 s: past about 470 new connections a second to the
//! one Redis address the server ran out of ports, and the flow guard failed closed. A
//! multiplexed connection carries any number of concurrent commands, so one is enough.
//!
//! [`SharedRedis::connection`] opens it on first use, not at startup, so the server still starts
//! while Redis is down and each caller sees the outage as an error, as before. Once open,
//! redis-rs reconnects it in the background after a drop.
//!
//! Blocking commands (`XREADGROUP BLOCK`, `BLPOP`) and pub/sub hold a connection until they
//! return, which would stall every caller sharing it: those keep connections of their own, from
//! [`SharedRedis::client`].

use std::sync::Arc;
use std::time::Duration;

use redis::RedisResult;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tokio::sync::OnceCell;

/// A failed open is reported at once and the next call tries again. redis-rs retries with a
/// backoff of at least 1 s a step (six steps by default), and every caller queues behind the
/// one attempt, so during an outage each would wait seconds to minutes instead of failing.
const OPEN_RETRIES: usize = 0;
/// An unreachable host fails within this, per attempt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// A command a hung Redis never answers fails after this instead of holding its caller (the
/// per-call connections this replaced had no timeout at all). Redis answers in well under 1 ms.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

/// A Redis client and the one shared connection opened from it. Cheap to clone: clones share
/// the connection.
#[derive(Clone)]
pub struct SharedRedis {
    client: redis::Client,
    connection: Arc<OnceCell<ConnectionManager>>,
}

impl SharedRedis {
    pub fn new(client: redis::Client) -> Self {
        Self {
            client,
            connection: Arc::new(OnceCell::new()),
        }
    }

    /// The shared connection, opened on the first call. A failed open is retried by the next
    /// call.
    pub async fn connection(&self) -> RedisResult<ConnectionManager> {
        self.connection
            .get_or_try_init(|| {
                let config = ConnectionManagerConfig::new()
                    .set_number_of_retries(OPEN_RETRIES)
                    .set_connection_timeout(CONNECT_TIMEOUT)
                    .set_response_timeout(RESPONSE_TIMEOUT);
                ConnectionManager::new_with_config(self.client.clone(), config)
            })
            .await
            .cloned()
    }

    /// The client, for a caller that needs a connection of its own (blocking reads, pub/sub).
    pub fn client(&self) -> &redis::Client {
        &self.client
    }
}

impl std::fmt::Debug for SharedRedis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRedis")
            .field("connected", &self.connection.initialized())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing listens on port 1: the open fails at once, is reported, and leaves nothing
    /// cached, so a later call tries again rather than holding the failure.
    #[tokio::test]
    async fn a_failed_open_is_an_error_at_once_and_is_retried() {
        let redis = SharedRedis::new(redis::Client::open("redis://127.0.0.1:1/").unwrap());
        let started = std::time::Instant::now();
        assert!(redis.connection().await.is_err());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "no backoff on a refused open"
        );
        assert!(!redis.connection.initialized());
        assert!(redis.connection().await.is_err());
    }

    #[test]
    fn clones_share_one_connection() {
        let redis = SharedRedis::new(redis::Client::open("redis://127.0.0.1:1/").unwrap());
        let clone = redis.clone();
        assert!(Arc::ptr_eq(&redis.connection, &clone.connection));
    }
}
