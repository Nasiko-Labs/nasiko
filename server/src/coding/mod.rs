//! Coding-agent (ACP) control-plane support.
//!
//! Two halves: [`lifecycle`] carries each coding-agent session's runtime state
//! and the one-active-turn lease on its `chat_sessions` row (the "registry"),
//! and [`turn`] runs a turn end to end (deploy → attach → ACP → stream). This
//! module root adds the control-plane reconciler that keeps the two honest
//! across restarts.

use std::collections::HashSet;
use std::time::Duration;

use crate::state::AppState;

pub mod entry;
pub mod lifecycle;
pub mod permission;
pub mod proxy;
pub mod revocation;
pub mod sse;
pub mod turn;

/// How often the control plane re-reconciles coding-agent session rows against
/// the containers that actually exist.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

/// Reset any session row that claims a live container which no longer exists,
/// clearing its stale turn lease. Returns the number of rows reset.
///
/// "Live" means the runtime still lists a container for the handle. We key on
/// the container id (which is what `set_runtime_state` records as
/// `runtime_handle` and what `ContainerRuntime::list` reports), so the two
/// always compare like for like.
pub async fn reconcile_once(state: &AppState) -> sqlx::Result<u64> {
    let live: HashSet<String> = match state.runtime.list().await {
        Ok(statuses) => statuses
            .into_iter()
            .map(|s| s.container_id.as_str().to_owned())
            .collect(),
        Err(e) => {
            // A transient runtime hiccup must not wrongly reset live sessions:
            // skip this sweep rather than treat every container as dead.
            tracing::warn!(error = %e, "coding-agent reconcile: runtime.list() failed; skipping sweep");
            return Ok(0);
        }
    };
    lifecycle::reconcile_startup(&state.db, &live).await
}

/// Spawn the control-plane reconciler: one sweep at boot — clearing the stale
/// leases a replica that died mid-turn would otherwise leave blocking the next
/// turn — then a sweep every [`RECONCILE_INTERVAL`].
///
/// This is the control-plane half of the timeout story; the complementary
/// in-container idle-exit wrapper (a container shutting itself down after sitting
/// idle) is deliberately deferred.
pub fn spawn_reconciler(state: AppState) {
    tokio::spawn(async move {
        loop {
            match reconcile_once(&state).await {
                Ok(n) if n > 0 => {
                    tracing::info!(
                        reset = n,
                        "coding-agent reconcile: reset stale session rows"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "coding-agent reconcile failed"),
            }
            tokio::time::sleep(RECONCILE_INTERVAL).await;
        }
    });
}
