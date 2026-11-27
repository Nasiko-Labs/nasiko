//! Run work on one server replica at a time.
//!
//! The server runs as several replicas, and every rollout briefly runs two even at one. Work that
//! must not run twice at once (a boot reconcile, a loop that creates clusters) takes a Postgres
//! advisory lock first with [`single_flight`]: whichever replica gets it does the work, the others
//! skip it. A periodic job that should run once a period across all replicas (a sweep that calls
//! the container runtime, a materializer) runs under [`every`], which elects one replica to lead it.
//!
//! An advisory lock rather than a Redis lease because the work it guards is database work and the
//! lock must end with its holder: a replica killed mid-pass loses its session, and Postgres releases
//! the lock with it, with no lease to expire.
//!
//! Keys are built with [`LockKey::named`] and use Postgres's two-`int4` lock form under one
//! namespace, a key space separate from the single-`bigint` locks the rest of the code takes, so a
//! name here can never collide with one of those.

use std::future::Future;
use std::time::Duration;

use sqlx::{Connection, PgConnection, PgPool};

/// The first half of every key: "nask".
const NAMESPACE: i32 = 0x6e61_736b;

/// An advisory lock key, named for the work it guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockKey {
    name: &'static str,
    id: i32,
}

impl LockKey {
    /// The key for `name`. Two keys with the same name are the same lock.
    pub const fn named(name: &'static str) -> Self {
        Self {
            name,
            id: fnv1a_32(name.as_bytes()) as i32,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// 32-bit FNV-1a, so a key's number is fixed by its name at compile time.
const fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

/// A held lock, on a connection of its own. Released by [`Self::release`], or by dropping it, which
/// closes the connection and so ends the session that holds the lock.
pub struct Held {
    key: LockKey,
    conn: Option<PgConnection>,
}

impl Held {
    pub async fn release(mut self) {
        if let Some(conn) = self.conn.take() {
            unlock(conn, self.key).await;
        }
    }
}

/// Take `key` if no other session holds it. `Ok(None)` when another replica has it.
///
/// The lock is taken on a new connection made with the pool's settings, not one of the pool's:
/// an advisory lock belongs to the session that took it, so its connection is pinned for as long
/// as the lock is held (a migration run, a leader's whole tenure). Drawn from the pool, that would
/// take a connection away from requests for that long, and with a pool of one (tests) the work it
/// guards could never get a connection at all.
pub async fn try_lock(pool: &PgPool, key: LockKey) -> Result<Option<Held>, sqlx::Error> {
    let mut conn = PgConnection::connect_with(&pool.connect_options()).await?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
        .bind(NAMESPACE)
        .bind(key.id)
        .fetch_one(&mut conn)
        .await?;
    if !got {
        let _ = conn.close().await;
        return Ok(None);
    }
    Ok(Some(Held {
        key,
        conn: Some(conn),
    }))
}

/// Wait until `key` is free and take it.
///
/// Polls [`try_lock`] instead of a blocking `pg_advisory_lock`: a session blocked in a statement
/// holds a snapshot, and `CREATE INDEX CONCURRENTLY` on the lock holder waits for every older
/// snapshot to end, so the two would wait on each other (migrations take this path).
pub async fn lock(pool: &PgPool, key: LockKey, poll: Duration) -> Result<Held, sqlx::Error> {
    loop {
        if let Some(held) = try_lock(pool, key).await? {
            return Ok(held);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Run `body` if this replica takes `key`; `None` (and `body` not run) when another holds it or
/// the lock could not be asked for.
pub async fn single_flight<F, Fut, T>(pool: &PgPool, key: LockKey, body: F) -> Option<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let held = match try_lock(pool, key).await {
        Ok(Some(held)) => held,
        Ok(None) => {
            tracing::debug!(lock = key.name, "another replica holds this lock; skipping");
            return None;
        }
        Err(e) => {
            tracing::warn!(lock = key.name, error = %e, "advisory lock unavailable; skipping");
            return None;
        }
    };
    let out = body().await;
    held.release().await;
    Some(out)
}

/// Run `job` every `period` on one replica: the leader for `key`. The first tick runs at once.
/// Never returns.
///
/// A replica that takes `key` keeps it, and runs every tick, for as long as its connection lives;
/// the others try again each period, so when the leader stops (its session ends and Postgres
/// releases the lock) one of them takes over within a period. Holding the lock across ticks,
/// rather than taking it per tick, is what keeps the job to once a period: per-tick locks only
/// stop two runs overlapping, and N replicas ticking at different moments would still run it N
/// times a period.
///
/// Before each tick the leader checks its connection is alive, since a dropped session has
/// already released the lock to someone else.
pub async fn every<F, Fut>(pool: PgPool, key: LockKey, period: Duration, mut job: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut leading: Option<Held> = None;
    loop {
        ticker.tick().await;
        leading = match leading.take() {
            Some(held) => still_held(held).await,
            None => match try_lock(&pool, key).await {
                Ok(Some(held)) => {
                    tracing::info!(lock = key.name, "leading this job on this replica");
                    Some(held)
                }
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(lock = key.name, error = %e, "advisory lock unavailable; skipping");
                    None
                }
            },
        };
        if leading.is_some() {
            job().await;
        }
    }
}

/// `held` back if its session is still alive (and so still holds the lock), else `None`.
async fn still_held(mut held: Held) -> Option<Held> {
    let conn = held.conn.as_mut()?;
    match sqlx::query("SELECT 1").execute(&mut *conn).await {
        Ok(_) => Some(held),
        Err(e) => {
            tracing::warn!(lock = held.key.name, error = %e, "lost the connection holding this lock; no longer leading");
            None
        }
    }
}

/// Unlock, then close the connection; closing ends the session, which releases the lock even if
/// the unlock failed.
async fn unlock(mut conn: PgConnection, key: LockKey) {
    let released: Result<bool, _> = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
        .bind(NAMESPACE)
        .bind(key.id)
        .fetch_one(&mut conn)
        .await;
    if !matches!(released, Ok(true)) {
        tracing::warn!(
            lock = key.name,
            "advisory lock not released; closing its connection"
        );
    }
    let _ = conn.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_fixed_by_name() {
        assert_eq!(LockKey::named("a"), LockKey::named("a"));
        assert_ne!(LockKey::named("a").id, LockKey::named("b").id);
        // FNV-1a test vector: "a" → 0xe40c292c.
        assert_eq!(fnv1a_32(b"a"), 0xe40c_292c);
    }
}
