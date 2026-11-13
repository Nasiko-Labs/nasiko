//! Ready replicas per agent, so the in-flight band (`agent_request.rs`) describes the load on one
//! replica rather than on the agent as a whole.
//!
//! The band feeds the per-agent capacity estimate. Measured agent-wide, an agent running N
//! replicas would look healthy at N times the load one replica can take, its estimate would rise
//! with every replica added, and anything sizing the agent from that estimate would ask for fewer
//! replicas than it needs.
//!
//! The ready count comes from [`ContainerRuntime::list`], which is too slow for the request
//! path. A background refresher ([`run`]) re-reads it every [`REFRESH_INTERVAL`] for the agents
//! that had a request within [`ACTIVE_WINDOW`], and the request path only reads the cached value.
//! An agent with no cached count yet, or a count of 0, is treated as one replica.

use dashmap::DashMap;
use nasiko_runtime::{ContainerId, ContainerRuntime};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

/// How often the ready counts of recently active agents are re-read.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(15);
/// How long after its last request an agent stays in the cache and keeps being refreshed.
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(5 * 60);

struct ReplicaSlot {
    /// Last ready count the runtime reported; 0 until the first refresh.
    ready: u32,
    last_request: Instant,
}

/// The cached ready-replica count of each recently active agent, keyed by agent UUID.
#[derive(Clone, Default)]
pub struct ReadyReplicas {
    agents: Arc<DashMap<String, ReplicaSlot>>,
}

impl ReadyReplicas {
    /// Record a request to `agent_id` at `now` and return the agent's ready replicas, at least 1.
    /// Never calls the runtime: an agent seen for the first time reads as one replica until the
    /// refresher has looked it up.
    pub fn note_request(&self, agent_id: &str, now: Instant) -> u64 {
        if let Some(mut slot) = self.agents.get_mut(agent_id) {
            slot.last_request = now;
            return u64::from(slot.ready.max(1));
        }
        self.agents.insert(
            agent_id.to_string(),
            ReplicaSlot {
                ready: 0,
                last_request: now,
            },
        );
        1
    }

    /// Forget the agents with no request within [`ACTIVE_WINDOW`] of `now`, so the cache is
    /// bounded by the agents currently in use, and return the ones that remain.
    fn retain_active(&self, now: Instant) -> Vec<String> {
        self.agents
            .retain(|_, slot| now.saturating_duration_since(slot.last_request) <= ACTIVE_WINDOW);
        self.agents.iter().map(|slot| slot.key().clone()).collect()
    }

    /// Store a freshly read ready count. An agent that left the cache meanwhile is not re-added.
    pub(crate) fn set_ready(&self, agent_id: &str, ready: u32) {
        if let Some(mut slot) = self.agents.get_mut(agent_id) {
            slot.ready = ready;
        }
    }
}

/// Re-read the ready count of every agent with a request within [`ACTIVE_WINDOW`] of `now`.
///
/// One [`ContainerRuntime::list`] for all of them, not a `status()` each: an agent counts as
/// active from its first request, which a new agent makes when it registers, so a `status()`
/// per agent cost about 20 calls per deploy (each up to four Kubernetes API calls) and, under
/// load, one per recently called agent every 15 s.
///
/// A failed list keeps every last known count, and so does an agent the list does not show: a
/// runtime hiccup should not change the band of every request in flight. Ids that are not UUIDs
/// are skipped, since agents are keyed by UUID at the runtime and anything else has no deployment
/// to ask about.
pub async fn refresh_once(replicas: &ReadyReplicas, runtime: &dyn ContainerRuntime, now: Instant) {
    let active = replicas.retain_active(now);
    if active.is_empty() {
        return;
    }
    let listed = match runtime.list().await {
        Ok(listed) => listed,
        Err(e) => {
            tracing::warn!(error = %e, "could not list ready replicas - keeping the last known counts");
            return;
        }
    };
    let ready: HashMap<&str, u32> = listed
        .iter()
        .map(|status| (status.container_id.as_str(), status.replicas_live))
        .collect();
    for agent_id in active {
        let Ok(uuid) = uuid::Uuid::parse_str(&agent_id) else {
            tracing::debug!(%agent_id, "agent id is not a UUID - its ready replicas are not read");
            continue;
        };
        if let Some(&count) = ready.get(ContainerId::from_uuid(uuid).as_str()) {
            replicas.set_ready(&agent_id, count);
        }
    }
}

/// Refresh the cache every [`REFRESH_INTERVAL`] for as long as the server runs.
pub async fn run(replicas: ReadyReplicas, runtime: Arc<dyn ContainerRuntime>) {
    let mut ticker = tokio::time::interval(REFRESH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        refresh_once(&replicas, runtime.as_ref(), Instant::now()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use nasiko_runtime::{DeploymentSpec, DeploymentStatus, RuntimeError, RuntimeState};
    use std::{collections::HashMap, sync::Mutex};

    const ACTIVE: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
    const IDLE: &str = "1a2b3c4d-5e6f-4789-8abc-def012345678";

    /// Lists a fixed ready count per agent, counting its lists and recording any `status()` call.
    #[derive(Default)]
    struct CountingRuntime {
        ready: HashMap<String, u32>,
        fail_list: bool,
        lists: Mutex<u32>,
        asked: Mutex<Vec<String>>,
    }

    fn unused<T>() -> nasiko_runtime::Result<T> {
        Err(RuntimeError::BackendUnreachable(
            "not used by the refresher".into(),
        ))
    }

    #[async_trait]
    impl ContainerRuntime for CountingRuntime {
        async fn deploy(&self, _spec: &DeploymentSpec) -> nasiko_runtime::Result<DeploymentStatus> {
            unused()
        }
        async fn destroy(&self, _id: &ContainerId) -> nasiko_runtime::Result<()> {
            unused()
        }
        async fn scale(&self, _id: &ContainerId, _replicas: u32) -> nasiko_runtime::Result<()> {
            unused()
        }
        async fn restart(&self, _id: &ContainerId) -> nasiko_runtime::Result<()> {
            unused()
        }
        async fn status(&self, id: &ContainerId) -> nasiko_runtime::Result<DeploymentStatus> {
            self.asked.lock().unwrap().push(id.as_str().to_string());
            unused()
        }
        async fn list(&self) -> nasiko_runtime::Result<Vec<DeploymentStatus>> {
            *self.lists.lock().unwrap() += 1;
            if self.fail_list {
                return unused();
            }
            Ok(self
                .ready
                .iter()
                .map(|(id, &replicas_live)| DeploymentStatus {
                    container_id: ContainerId::new(id.as_str()),
                    state: RuntimeState::Running,
                    replicas_live,
                    endpoint: None,
                    message: None,
                    restart_count: 0,
                })
                .collect())
        }
        async fn endpoint(&self, _id: &ContainerId) -> nasiko_runtime::Result<String> {
            unused()
        }
        async fn logs(&self, _id: &ContainerId, _tail: u32) -> nasiko_runtime::Result<Vec<String>> {
            unused()
        }
        async fn build(
            &self,
            _context: &[u8],
            _tag: &str,
        ) -> nasiko_runtime::Result<nasiko_runtime::BuildReport> {
            unused()
        }
    }

    fn runtime_with(ready: &[(&str, u32)]) -> CountingRuntime {
        CountingRuntime {
            ready: ready.iter().map(|(id, n)| (id.to_string(), *n)).collect(),
            ..CountingRuntime::default()
        }
    }

    const OTHER: &str = "2b3c4d5e-6f70-4812-9abc-0123456789ab";

    #[test]
    fn an_agent_with_no_cached_count_reads_as_one_replica() {
        let replicas = ReadyReplicas::default();
        assert_eq!(replicas.note_request(ACTIVE, Instant::now()), 1);
    }

    #[test]
    fn a_cached_count_of_zero_reads_as_one_replica() {
        let replicas = ReadyReplicas::default();
        let now = Instant::now();
        replicas.note_request(ACTIVE, now);
        replicas.set_ready(ACTIVE, 0);
        assert_eq!(replicas.note_request(ACTIVE, now), 1);
        replicas.set_ready(ACTIVE, 3);
        assert_eq!(replicas.note_request(ACTIVE, now), 3);
    }

    #[tokio::test]
    async fn the_refresher_reads_only_recently_active_agents() {
        let replicas = ReadyReplicas::default();
        let start = Instant::now();
        replicas.note_request(IDLE, start);
        let later = start + ACTIVE_WINDOW + Duration::from_secs(60);
        replicas.note_request(ACTIVE, later);
        let runtime = runtime_with(&[(ACTIVE, 2), (IDLE, 5)]);

        refresh_once(&replicas, &runtime, later).await;

        assert_eq!(replicas.note_request(ACTIVE, later), 2);
        // The idle agent left the cache: its next request starts from one replica again.
        assert_eq!(replicas.note_request(IDLE, later), 1);
    }

    #[tokio::test]
    async fn one_list_serves_every_active_agent_and_status_is_never_called() {
        let replicas = ReadyReplicas::default();
        let now = Instant::now();
        replicas.note_request(ACTIVE, now);
        replicas.note_request(OTHER, now);
        let runtime = runtime_with(&[(ACTIVE, 2), (OTHER, 3)]);

        refresh_once(&replicas, &runtime, now).await;

        assert_eq!(*runtime.lists.lock().unwrap(), 1);
        assert!(runtime.asked.lock().unwrap().is_empty());
        assert_eq!(replicas.note_request(ACTIVE, now), 2);
        assert_eq!(replicas.note_request(OTHER, now), 3);
    }

    #[tokio::test]
    async fn no_active_agent_means_no_list() {
        let runtime = runtime_with(&[(ACTIVE, 2)]);
        refresh_once(&ReadyReplicas::default(), &runtime, Instant::now()).await;
        assert_eq!(*runtime.lists.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn a_failed_list_keeps_the_last_known_count() {
        let replicas = ReadyReplicas::default();
        let now = Instant::now();
        replicas.note_request(ACTIVE, now);
        replicas.set_ready(ACTIVE, 4);
        let runtime = CountingRuntime {
            fail_list: true,
            ..runtime_with(&[(ACTIVE, 2)])
        };

        refresh_once(&replicas, &runtime, now).await;

        assert_eq!(replicas.note_request(ACTIVE, now), 4);
    }

    #[tokio::test]
    async fn an_agent_missing_from_the_list_keeps_its_last_known_count() {
        let replicas = ReadyReplicas::default();
        let now = Instant::now();
        replicas.note_request(ACTIVE, now);
        replicas.set_ready(ACTIVE, 4);

        refresh_once(&replicas, &runtime_with(&[]), now).await;

        assert_eq!(replicas.note_request(ACTIVE, now), 4);
    }

    #[tokio::test]
    async fn an_id_that_is_not_a_uuid_is_never_looked_up() {
        let replicas = ReadyReplicas::default();
        let now = Instant::now();
        replicas.note_request("my-agent", now);
        let runtime = runtime_with(&[("my-agent", 3)]);

        refresh_once(&replicas, &runtime, now).await;

        assert_eq!(replicas.note_request("my-agent", now), 1);
    }
}
