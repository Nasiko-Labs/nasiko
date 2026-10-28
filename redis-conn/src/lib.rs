//! One shared Redis connection per server, for the request path.
//!
//! `redis::Client::get_multiplexed_async_connection()` opens a **new TCP connection every call**.
//! The chat path makes about eight short Redis calls per request (flow guard, MCP gateway cache,
//! memory cache and flags), so under load each request paid eight connection setups, and at tens
//! of requests a second the connect queue backed up: connects took seconds, and requests stalled
//! well before CPU, Postgres or Redis were busy.
//!
//! [`connection`] returns a clone of one [`ConnectionManager`] per Redis server instead. A clone
//! shares the single underlying multiplexed connection (commands from many tasks pipeline over
//! it), and the manager reconnects by itself if Redis restarts, so callers keep their existing
//! error handling and never hold a dead connection.
//!
//! **Not for blocking commands** (`XREADGROUP ... BLOCK`, `BLPOP`, pub/sub): they would stall every
//! other caller sharing the connection. Those keep a connection of their own.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use redis::RedisResult;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tokio::runtime::Id as RuntimeId;
use tokio::sync::OnceCell;

/// A manager spawns its driver task on the runtime that created it and stops when that runtime
/// ends, so managers are kept per runtime. A server has one; each `#[tokio::test]` has its own,
/// and must not inherit a manager whose runtime already shut down.
type Key = (RuntimeId, String);

static MANAGERS: LazyLock<Mutex<HashMap<Key, Arc<OnceCell<ConnectionManager>>>>> =
    LazyLock::new(Default::default);

/// The shared connection to `client`'s server, created on first use.
///
/// Must be called from inside a tokio runtime, like every other Redis call here. Errors are the
/// ones `get_multiplexed_async_connection` returned, so existing fail-open and fail-closed
/// handling at the call sites is unchanged.
pub async fn connection(client: &redis::Client) -> RedisResult<ConnectionManager> {
    let cell = {
        let key = (tokio::runtime::Handle::current().id(), server_key(client));
        let mut managers = MANAGERS.lock().unwrap_or_else(|e| e.into_inner());
        managers.entry(key).or_default().clone()
    };
    cell.get_or_try_init(|| ConnectionManager::new_with_config(client.clone(), config()))
        .await
        .cloned()
}

/// Fail fast, as `get_multiplexed_async_connection` did. The manager's default retries a failed
/// connect six times with growing delays (about six seconds), which would make every flow-guard
/// and cache call wait that long while Redis is down instead of failing at once and letting the
/// caller apply its own fail-open or fail-closed rule. A failed attempt is simply tried again by
/// the next call, and a connect that never answers gives up after a few seconds rather than the
/// operating system's much longer timeout.
fn config() -> ConnectionManagerConfig {
    ConnectionManagerConfig::new()
        .set_number_of_retries(0)
        .set_connection_timeout(CONNECT_TIMEOUT)
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Which server a client talks to: address, database and credentials, so two clients for the same
/// server share a connection and clients for different servers (or databases) never do. Used only
/// as an in-memory map key, never logged.
fn server_key(client: &redis::Client) -> String {
    format!("{:?}", client.get_connection_info())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(url: &str) -> redis::Client {
        redis::Client::open(url).expect("a well-formed redis url")
    }

    #[test]
    fn clients_for_the_same_server_share_a_key() {
        assert_eq!(
            server_key(&client("redis://localhost:6379/15")),
            server_key(&client("redis://localhost:6379/15"))
        );
    }

    #[test]
    fn a_different_database_or_host_gets_its_own_key() {
        let base = server_key(&client("redis://localhost:6379/15"));
        assert_ne!(base, server_key(&client("redis://localhost:6379/0")));
        assert_ne!(base, server_key(&client("redis://other-host:6379/15")));
    }
}
