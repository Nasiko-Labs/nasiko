//! The HITL continuation buffer across replicas: the resume writes on the replica that claimed
//! the row, the browser's reconnect reads on another. Two `ContinuationRegistry`s, each with its
//! own Redis connection, stand for the two replicas (`oss/server/src/hitl/continuation.rs`).
//!
//! `just infra` then `cargo test -p nasiko-server --test hitl_continuation_replicas`

use std::time::Duration;

use futures::StreamExt;
use nasiko_server::hitl::continuation::{ContinuationGuard, ContinuationRegistry};
use uuid::Uuid;

/// Long enough for any Redis round trip here; a watch that should end but hangs fails at this.
const WATCH_LIMIT: Duration = Duration::from_secs(10);

fn replica() -> ContinuationRegistry {
    let url = std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into());
    ContinuationRegistry::new(nasiko_redis::SharedRedis::new(
        redis::Client::open(url).unwrap(),
    ))
}

async fn collect(registry: &ContinuationRegistry, id: Uuid) -> Vec<String> {
    tokio::time::timeout(WATCH_LIMIT, registry.watch(id).collect::<Vec<_>>())
        .await
        .expect("the watch ends at the terminal marker")
}

/// Lets the guard's writer task flush to Redis after the guard is dropped.
async fn written() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn a_reconnect_on_another_replica_after_the_resume_finished_replays_it_all() {
    let (resuming, reconnecting) = (replica(), replica());
    let id = Uuid::new_v4();
    {
        let guard = ContinuationGuard::open(&resuming, id).await;
        guard.push("one".into());
        guard.push("two".into());
    }
    written().await;

    assert!(reconnecting.exists(id).await);
    assert_eq!(collect(&reconnecting, id).await, ["one", "two"]);
}

#[tokio::test]
async fn a_reconnect_on_another_replica_tails_a_resume_still_running() {
    let (resuming, reconnecting) = (replica(), replica());
    let id = Uuid::new_v4();
    let guard = ContinuationGuard::open(&resuming, id).await;
    guard.push("before".into());
    written().await;

    let watcher = tokio::spawn({
        let reconnecting = reconnecting.clone();
        async move { collect(&reconnecting, id).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    guard.push("after".into());
    drop(guard);

    assert_eq!(watcher.await.unwrap(), ["before", "after"]);
}

#[tokio::test]
async fn a_reconnect_that_arrives_before_the_resume_starts_waits_for_it() {
    let (resuming, reconnecting) = (replica(), replica());
    let id = Uuid::new_v4();
    let watcher = tokio::spawn({
        let reconnecting = reconnecting.clone();
        async move { collect(&reconnecting, id).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    ContinuationGuard::open(&resuming, id)
        .await
        .push("late start".into());

    assert_eq!(watcher.await.unwrap(), ["late start"]);
}

#[tokio::test]
async fn an_alias_made_on_one_replica_resolves_on_another() {
    let (resolving, reconnecting) = (replica(), replica());
    let (mcp_row, mirror_row) = (Uuid::new_v4(), Uuid::new_v4());
    assert!(!reconnecting.exists(mcp_row).await, "nothing claimed yet");

    resolving.alias(mcp_row, mirror_row).await;
    assert!(
        reconnecting.exists(mcp_row).await,
        "claimed as soon as it is aliased"
    );

    ContinuationGuard::open(&resolving, mirror_row)
        .await
        .push("through the alias".into());
    written().await;

    assert_eq!(collect(&reconnecting, mcp_row).await, ["through the alias"]);
}

#[tokio::test]
async fn an_id_nothing_claimed_does_not_exist() {
    assert!(!replica().exists(Uuid::new_v4()).await);
}
