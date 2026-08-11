//! Resource usage (CPU / memory / disk) for the things running on a Nasiko box.
//!
//! This is a separate seam from [`ContainerRuntime`](crate::ContainerRuntime) on
//! purpose. That trait is agent-scoped and UUID-keyed (`ContainerId::from_uuid`),
//! but half of what an admin needs to see — Postgres, Redis, rustfs, the control
//! plane itself — has no agent row at all. Widening `ContainerRuntime` to cover
//! them would mean inventing `ContainerId`s for things that are not agents.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::Result;

/// Resolves an agent UUID to the name an operator recognises.
///
/// A trait so the runtime crate stays free of any database dependency — the
/// server supplies the lookup, tests supply a map.
#[async_trait]
pub trait AgentNameResolver: Send + Sync {
    /// Display names by agent UUID. Missing entries fall back to the raw name.
    async fn resolve(&self, agent_ids: &[String]) -> HashMap<String, String>;
}

/// Which part of the platform a container belongs to.
///
/// The three groups an operator actually reasons about: "is the control plane
/// healthy", "are the agents behaving", "is the supporting infrastructure OK".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsGroup {
    /// The control-plane server itself.
    ControlPlane,
    /// A deployed agent.
    AgentRuntime,
    /// Postgres, Redis, rustfs, Caddy, Tempo, Loki, the OTel collector — anything
    /// the platform depends on but does not route user traffic to.
    Infra,
}

/// Host-level totals. Everything is a point-in-time reading; nothing is retained.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct HostStats {
    /// CPUs visible to the container engine.
    pub cpu_count: u64,
    /// Total physical memory on the host.
    pub mem_total_bytes: u64,
    /// Disk consumed by images (shared layers counted once).
    pub docker_images_bytes: u64,
    /// Disk consumed by local volumes.
    pub docker_volumes_bytes: u64,
    /// Of the above, how much could be freed by a prune.
    pub docker_reclaimable_bytes: u64,
    /// Total size of the filesystem backing the container engine.
    ///
    /// `None` unless the host root is mounted into the server — the Docker API
    /// cannot report it. See `disk_source` on [`PlatformStats`].
    pub disk_total_bytes: Option<u64>,
    /// Used bytes on that filesystem. `None` under the same condition.
    pub disk_used_bytes: Option<u64>,
}

/// One container's usage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContainerStats {
    /// Engine-level name, e.g. `nasiko-agent-<uuid>`.
    pub name: String,
    /// Human-facing name: an agent's registered name where one could be
    /// resolved, otherwise the compose service name, otherwise `name`.
    pub display_name: String,
    pub group: StatsGroup,
    /// Engine state string (`running`, `restarting`, `exited`, …).
    pub state: String,
    /// Percent of one-CPU-equivalent, so 250.0 means 2.5 cores on a 4-core box.
    ///
    /// `None` when it could not be derived from a single sample rather than 0.0 —
    /// reporting a hard zero for "unknown" reads as an idle container.
    pub cpu_percent: Option<f64>,
    pub mem_used_bytes: u64,
    /// The container's memory ceiling. Equals host memory when unconstrained,
    /// which is the usual case here — no agent sets a limit by default.
    pub mem_limit_bytes: u64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub block_read_bytes: u64,
    pub block_write_bytes: u64,
}

/// Where [`HostStats::disk_total_bytes`] came from, so the UI can say why the
/// number is missing instead of rendering a blank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskSource {
    /// True filesystem usage, read from a mounted host path.
    Host,
    /// Docker's own accounting only — host total/used unavailable.
    Docker,
}

/// A complete reading: host totals plus every container, grouped.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlatformStats {
    pub host: HostStats,
    pub containers: Vec<ContainerStats>,
    pub disk_source: DiskSource,
}

/// Reads resource usage from whatever runs the workloads.
///
/// Implemented over the Docker daemon in OSS and over `metrics.k8s.io` in EE.
#[async_trait]
pub trait ResourceStatsProvider: Send + Sync {
    /// One point-in-time reading of the whole box. Admin-scoped: it necessarily
    /// reveals the deployment's shape and the host's size.
    async fn platform_stats(&self) -> Result<PlatformStats>;

    /// Usage for one agent, addressed by its UUID.
    ///
    /// Deliberately narrow so an agent's owner can be shown their own agent's
    /// usage without being shown the host or anybody else's containers. It is a
    /// distinct method rather than a filter over [`platform_stats`] for that
    /// reason and one more: filtering would sample every container on the box to
    /// return one, and this endpoint is polled by a per-agent page.
    ///
    /// `Ok(None)` means the agent has no container right now — not an error; a
    /// scaled-to-zero or never-deployed agent is a normal state.
    async fn agent_stats(&self, agent_id: &str) -> Result<Option<ContainerStats>>;
}

/// Stand-in for runtimes that cannot report usage (the simulated runtime, and
/// Kubernetes until EE's provider is wired).
///
/// Fails the call with a plain explanation instead of returning zeroed stats,
/// which would render as a healthy idle box.
#[derive(Debug, Default)]
pub struct UnsupportedStatsProvider {
    /// Name of the runtime that cannot report, for the error message.
    pub runtime: String,
}

impl UnsupportedStatsProvider {
    fn unsupported<T>(&self) -> Result<T> {
        Err(crate::RuntimeError::Internal(format!(
            "resource stats are not available for the '{}' runtime",
            self.runtime
        )))
    }
}

#[async_trait]
impl ResourceStatsProvider for UnsupportedStatsProvider {
    async fn platform_stats(&self) -> Result<PlatformStats> {
        self.unsupported()
    }

    async fn agent_stats(&self, _agent_id: &str) -> Result<Option<ContainerStats>> {
        self.unsupported()
    }
}

/// Serves one whole-box reading from cache, refreshing it in the background
/// rather than making a caller wait for the sweep.
///
/// [`ResourceStatsProvider::platform_stats`] costs one Docker `stats` call per
/// container, and each of those blocks about a second while the daemon takes the
/// second CPU sample the delta needs — so a sweep costs ~1-2s no matter how much
/// of it runs concurrently.
///
/// A plain read-through TTL cache does not help a page that polls: with the TTL
/// equal to the poll interval, every poll arrives just after the entry expired
/// and pays the full sweep, which is exactly the "Resources takes seconds"
/// symptom. So a stale reading is served immediately and a refresh is kicked off
/// behind it: only the very first request after startup waits for Docker, and
/// every later one is answered from memory with data at most one poll old.
///
/// Refreshes are single-flight — a burst of pollers triggers one sweep, not one
/// each, which matters because they contend for the same Docker socket the
/// control plane deploys agents through.
///
/// Only `platform_stats` is wrapped. `agent_stats` samples exactly one container
/// and is loaded once per agent-card view rather than polled, so keying a cache
/// per agent would add bookkeeping for no measurable gain.
pub struct CachedStatsProvider<P> {
    shared: Arc<CacheShared<P>>,
}

struct CacheShared<P> {
    inner: P,
    /// How long a reading is served without triggering a refresh behind it.
    ttl: Duration,
    cached: Mutex<Option<(Instant, PlatformStats)>>,
    /// Set while a background refresh is in flight, so concurrent callers don't
    /// each spawn their own sweep.
    refreshing: AtomicBool,
}

impl<P> CachedStatsProvider<P> {
    pub fn new(inner: P, ttl: Duration) -> Self {
        Self {
            shared: Arc::new(CacheShared {
                inner,
                ttl,
                cached: Mutex::new(None),
                refreshing: AtomicBool::new(false),
            }),
        }
    }
}

impl<P: ResourceStatsProvider + 'static> CachedStatsProvider<P> {
    /// Starts one background sweep, unless one is already running.
    ///
    /// The `swap` is the single-flight gate: only the caller that flips the flag
    /// from false to true owns the refresh, so a burst of pollers triggers one
    /// sweep rather than one each.
    fn spawn_refresh(&self) {
        if self.shared.refreshing.swap(true, Ordering::AcqRel) {
            return;
        }
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            match shared.inner.platform_stats().await {
                Ok(fresh) => *shared.cached.lock().await = Some((Instant::now(), fresh)),
                // Leave the previous reading in place and let the next poll try
                // again — a brief daemon hiccup shouldn't blank the page.
                Err(e) => tracing::debug!(error = %e, "background resource-stats refresh failed"),
            }
            shared.refreshing.store(false, Ordering::Release);
        });
    }
}

impl<P> std::fmt::Debug for CachedStatsProvider<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedStatsProvider")
            .field("ttl", &self.shared.ttl)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl<P: ResourceStatsProvider + 'static> ResourceStatsProvider for CachedStatsProvider<P> {
    async fn platform_stats(&self) -> Result<PlatformStats> {
        // Warm path: answer from memory, and if the reading has aged past the TTL
        // start a refresh behind the response rather than in front of it.
        {
            let slot = self.shared.cached.lock().await;
            if let Some((taken_at, stats)) = slot.as_ref() {
                let stats = stats.clone();
                let stale = taken_at.elapsed() >= self.shared.ttl;
                drop(slot);
                if stale {
                    self.spawn_refresh();
                }
                return Ok(stats);
            }
        }

        // Cold: there is nothing to serve, so this caller does wait for the
        // sweep. The lock is held across it — as it always was — so a burst of
        // callers arriving on an empty cache collapses onto one sweep instead of
        // each starting its own.
        let mut slot = self.shared.cached.lock().await;
        if let Some((_, stats)) = slot.as_ref() {
            // Another caller filled the slot while this one waited for the lock.
            return Ok(stats.clone());
        }
        // Failures are deliberately not cached: a daemon that was briefly
        // unreachable must not read as unreachable for the whole TTL.
        let fresh = self.shared.inner.platform_stats().await?;
        *slot = Some((Instant::now(), fresh.clone()));
        Ok(fresh)
    }

    async fn agent_stats(&self, agent_id: &str) -> Result<Option<ContainerStats>> {
        self.shared.inner.agent_stats(agent_id).await
    }
}

/// Classifies a container into a [`StatsGroup`].
///
/// Agents are matched on the `nasiko-agent-` prefix because `DockerRuntime`
/// names them deterministically and creates them through the engine directly, so
/// they carry no compose labels. Everything else is a compose service, and the
/// service name — not the container name — is what identifies it, since the
/// container name embeds a project prefix that changes with the deploy directory.
///
/// `name` may arrive with the leading `/` the Docker list API adds.
///
/// The prefix is anchored deliberately: a bare `contains` would classify
/// `old-nasiko-agent-foo` as an agent, the same trap the list filter at
/// `docker/mod.rs` guards against with `^/nasiko-agent-`.
pub fn classify(name: &str, compose_service: Option<&str>) -> StatsGroup {
    let bare = name.strip_prefix('/').unwrap_or(name);
    if bare.starts_with("nasiko-agent-") {
        return StatsGroup::AgentRuntime;
    }
    match compose_service {
        Some("server") => StatsGroup::ControlPlane,
        // Unlabelled and not an agent: something started outside the stack. Infra
        // is the honest bucket — it is on the box consuming resources either way.
        _ => StatsGroup::Infra,
    }
}

/// Extracts the agent UUID from a container name, if it is an agent container.
pub fn agent_id_from_name(name: &str) -> Option<&str> {
    let bare = name.strip_prefix('/').unwrap_or(name);
    bare.strip_prefix("nasiko-agent-")
}

/// CPU percent from two cumulative samples, as `docker stats` computes it.
///
/// Returns `None` rather than `0.0` when the deltas cannot support a figure —
/// a zero `system_delta` means the two samples are the same instant (or
/// `precpu_stats` was never populated, which is what `one_shot: true` produces).
/// Callers must not paper over that with a default: a container pinning a core
/// and a container doing nothing would look identical.
pub fn cpu_percent(
    cpu_total: u64,
    precpu_total: u64,
    system_total: u64,
    presystem_total: u64,
    online_cpus: u64,
) -> Option<f64> {
    let cpu_delta = cpu_total.checked_sub(precpu_total)?;
    let system_delta = system_total.checked_sub(presystem_total)?;
    if system_delta == 0 || online_cpus == 0 {
        return None;
    }
    Some((cpu_delta as f64 / system_delta as f64) * online_cpus as f64 * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts sweeps so a test can assert how many reached the inner provider.
    /// `delay` makes a sweep long enough for a second caller to arrive mid-flight.
    struct FakeProvider {
        calls: AtomicUsize,
        fail_first: bool,
        delay: Option<Duration>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_first: false,
                delay: None,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ResourceStatsProvider for FakeProvider {
        async fn platform_stats(&self) -> Result<PlatformStats> {
            let nth = self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            if self.fail_first && nth == 0 {
                return Err(crate::RuntimeError::BackendUnreachable("down".into()));
            }
            Ok(PlatformStats {
                host: HostStats::default(),
                containers: Vec::new(),
                disk_source: DiskSource::Docker,
            })
        }

        async fn agent_stats(&self, _agent_id: &str) -> Result<Option<ContainerStats>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn reading_is_reused_within_the_ttl() {
        let cached = CachedStatsProvider::new(FakeProvider::new(), Duration::from_secs(60));
        cached.platform_stats().await.expect("first sweep");
        cached.platform_stats().await.expect("served from cache");
        assert_eq!(cached.shared.inner.calls(), 1);
    }

    #[tokio::test]
    async fn stale_reading_is_served_without_waiting_for_the_sweep() {
        // The property the Resources page depends on: once there is any reading,
        // a caller is answered from memory even when it has expired. Previously
        // an expired entry made the caller wait for a full per-container sweep,
        // and with the TTL equal to the poll interval that was every poll.
        let inner = FakeProvider {
            delay: Some(Duration::from_millis(300)),
            ..FakeProvider::new()
        };
        let cached = CachedStatsProvider::new(inner, Duration::ZERO);
        cached.platform_stats().await.expect("first sweep");

        let started = Instant::now();
        cached.platform_stats().await.expect("served stale");
        assert!(
            started.elapsed() < Duration::from_millis(150),
            "expired reading should be served from memory, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn stale_read_refreshes_in_the_background() {
        // Serving stale is only correct if the reading actually gets renewed.
        let cached = CachedStatsProvider::new(FakeProvider::new(), Duration::ZERO);
        cached.platform_stats().await.expect("first sweep");
        assert_eq!(cached.shared.inner.calls(), 1);

        cached.platform_stats().await.expect("served stale");
        // The refresh is spawned, so yield until it lands rather than asserting
        // on a sleep.
        for _ in 0..50 {
            if cached.shared.inner.calls() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(cached.shared.inner.calls(), 2, "refresh did not run");
    }

    #[tokio::test]
    async fn a_burst_of_stale_reads_triggers_one_refresh() {
        // Single-flight on the background path too: many pollers hitting an
        // expired reading must not each start a sweep against the Docker socket.
        let inner = FakeProvider {
            delay: Some(Duration::from_millis(100)),
            ..FakeProvider::new()
        };
        let cached = CachedStatsProvider::new(inner, Duration::ZERO);
        cached.platform_stats().await.expect("first sweep");

        for _ in 0..5 {
            cached.platform_stats().await.expect("served stale");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            cached.shared.inner.calls(),
            2,
            "five stale reads should share one refresh"
        );
    }

    #[tokio::test]
    async fn concurrent_callers_collapse_onto_one_sweep() {
        // The load amplification this type exists to prevent: three tabs polling
        // at once must cost one per-container fan-out, not three.
        let inner = FakeProvider {
            delay: Some(Duration::from_millis(50)),
            ..FakeProvider::new()
        };
        let cached = CachedStatsProvider::new(inner, Duration::from_secs(60));
        let (a, b, c) = tokio::join!(
            cached.platform_stats(),
            cached.platform_stats(),
            cached.platform_stats(),
        );
        assert!(a.is_ok() && b.is_ok() && c.is_ok());
        assert_eq!(cached.shared.inner.calls(), 1);
    }

    #[tokio::test]
    async fn failures_are_not_cached() {
        // A transient daemon failure must not stick for the whole TTL, so the
        // next caller retries rather than being handed the error again.
        let inner = FakeProvider {
            fail_first: true,
            ..FakeProvider::new()
        };
        let cached = CachedStatsProvider::new(inner, Duration::from_secs(60));
        assert!(cached.platform_stats().await.is_err());
        assert!(cached.platform_stats().await.is_ok());
        assert_eq!(cached.shared.inner.calls(), 2);
    }

    #[tokio::test]
    async fn agent_stats_is_not_cached() {
        let cached = CachedStatsProvider::new(FakeProvider::new(), Duration::from_secs(60));
        assert!(cached.agent_stats("some-id").await.expect("ok").is_none());
        // Passing through must not populate the whole-box slot.
        assert_eq!(cached.shared.inner.calls(), 0);
    }

    #[test]
    fn classifies_agents_by_anchored_prefix() {
        assert_eq!(
            classify("/nasiko-agent-446ee8d3-c4bf", None),
            StatsGroup::AgentRuntime
        );
        assert_eq!(
            classify("nasiko-agent-446ee8d3-c4bf", None),
            StatsGroup::AgentRuntime
        );
    }

    #[test]
    fn does_not_classify_substring_match_as_agent() {
        // The `^/nasiko-agent-` anchoring trap: this must not be an agent.
        assert_eq!(classify("/old-nasiko-agent-foo", None), StatsGroup::Infra);
    }

    #[test]
    fn classifies_server_service_as_control_plane() {
        assert_eq!(
            classify("/nasiko-server-1", Some("server")),
            StatsGroup::ControlPlane
        );
    }

    #[test]
    fn classifies_supporting_services_as_infra() {
        for svc in ["postgres", "redis", "rustfs", "caddy", "tempo", "loki"] {
            assert_eq!(
                classify(&format!("/nasiko-{svc}-1"), Some(svc)),
                StatsGroup::Infra
            );
        }
    }

    #[test]
    fn unlabelled_non_agent_container_is_infra() {
        assert_eq!(classify("/some-stray-container", None), StatsGroup::Infra);
    }

    #[test]
    fn extracts_agent_id() {
        assert_eq!(
            agent_id_from_name("/nasiko-agent-446ee8d3-c4bf"),
            Some("446ee8d3-c4bf")
        );
        assert_eq!(agent_id_from_name("/nasiko-postgres-1"), None);
    }

    #[test]
    fn cpu_percent_scales_by_online_cpus() {
        // 10% of the system delta on a 2-CPU box = 20% of one-CPU-equivalent.
        let pct = cpu_percent(100, 0, 1000, 0, 2).expect("derivable");
        assert!((pct - 20.0).abs() < f64::EPSILON, "got {pct}");
    }

    #[test]
    fn cpu_percent_is_none_when_system_delta_is_zero() {
        // What `one_shot: true` yields — must be None, never 0.0.
        assert_eq!(cpu_percent(100, 0, 0, 0, 2), None);
        assert_eq!(cpu_percent(100, 100, 500, 500, 2), None);
    }

    #[test]
    fn cpu_percent_is_none_without_online_cpus() {
        assert_eq!(cpu_percent(100, 0, 1000, 0, 0), None);
    }

    #[test]
    fn cpu_percent_is_none_on_counter_reset() {
        // Cumulative counters going backwards (daemon restart) must not wrap.
        assert_eq!(cpu_percent(0, 100, 1000, 0, 2), None);
        assert_eq!(cpu_percent(100, 0, 0, 1000, 2), None);
    }
}
