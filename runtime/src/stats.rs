//! Resource usage (CPU / memory / disk) for the things running on a Nasiko box.
//!
//! This is a separate seam from [`ContainerRuntime`](crate::ContainerRuntime) on
//! purpose. That trait is agent-scoped and UUID-keyed (`ContainerId::from_uuid`),
//! but half of what an admin needs to see — Postgres, Redis, rustfs, the control
//! plane itself — has no agent row at all. Widening `ContainerRuntime` to cover
//! them would mean inventing `ContainerId`s for things that are not agents.

use std::collections::HashMap;

use async_trait::async_trait;

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
