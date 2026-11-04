//! Redis cache helpers.
//!
//! Thin JSON get/set/delete over the shared `redis::Client` (the same client
//! `AppState` already holds). All operations **degrade gracefully**: a Redis
//! outage is treated as a cache miss (get returns `None`) or a no-op (set/delete
//! log and continue) rather than failing the request — the gateway stays up if
//! Redis blips, just slower.
//!
//! Key namespaces (all under `mcp:`) — see plan §10:
//!   * `mcp:session:{user}`   — resolved Composio backend (url+headers+toolkits)
//!   * `mcp:perm:{user}:{agent}` — serialized PermissionContext (Step 6)
//!   * `mcp:manifest:{…}`     — merged tool manifest (Step 6)

use redis::AsyncCommands;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Fetch and deserialize a JSON value. Returns `None` on miss, deserialize
/// failure, or any Redis error (logged) — callers treat all three as "recompute".
pub async fn get_json<T: DeserializeOwned>(client: &redis::Client, key: &str) -> Option<T> {
    let mut conn = match client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, key, "redis unavailable (cache get) — treating as miss");
            return None;
        }
    };
    let raw: Option<String> = match conn.get(key).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, key, "redis GET failed — treating as miss");
            return None;
        }
    };
    raw.and_then(|s| match serde_json::from_str(&s) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(error = %e, key, "cached value failed to deserialize — treating as miss");
            None
        }
    })
}

/// Serialize and store a JSON value with a TTL (seconds). Errors are logged and
/// swallowed — a failed cache write must never fail the request.
pub async fn set_json_ex<T: Serialize>(
    client: &redis::Client,
    key: &str,
    value: &T,
    ttl_secs: u64,
) {
    let payload = match serde_json::to_string(value) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, key, "cache value failed to serialize — skipping write");
            return;
        }
    };
    let mut conn = match client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, key, "redis unavailable (cache set) — skipping write");
            return;
        }
    };
    if let Err(e) = conn.set_ex::<_, _, ()>(key, payload, ttl_secs).await {
        tracing::warn!(error = %e, key, "redis SET failed — cache not updated");
    }
}

/// Delete a key. Errors are logged and swallowed.
pub async fn delete(client: &redis::Client, key: &str) {
    let mut conn = match client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, key, "redis unavailable (cache delete) — skipping");
            return;
        }
    };
    if let Err(e) = conn.del::<_, ()>(key).await {
        tracing::warn!(error = %e, key, "redis DEL failed");
    }
}
