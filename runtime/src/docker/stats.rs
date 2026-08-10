//! [`ResourceStatsProvider`] over the Docker daemon.
//!
//! Covers the whole box in the Compose topology: the control-plane server, every
//! agent container, and the supporting services (Postgres, Redis, rustfs, Caddy,
//! Tempo, Loki, the OTel collector). The Kubernetes topology is served by EE's
//! provider instead — there is no mixed deployment to straddle.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{ListContainersOptions, Stats, StatsOptions};
use futures_util::StreamExt;
use tracing::warn;

use crate::error::{Result, RuntimeError};
use crate::stats::{
    AgentNameResolver, ContainerStats, DiskSource, HostStats, PlatformStats, ResourceStatsProvider,
    classify, cpu_percent,
};

/// Compose stamps this on every service container it creates.
const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";

/// Reads usage from the local Docker daemon.
pub struct DockerStatsProvider {
    client: Docker,
    names: Option<Arc<dyn AgentNameResolver>>,
}

impl std::fmt::Debug for DockerStatsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerStatsProvider")
            .finish_non_exhaustive()
    }
}

impl DockerStatsProvider {
    /// Build over an existing daemon connection.
    pub fn new(client: Docker) -> Self {
        Self {
            client,
            names: None,
        }
    }

    /// Connect to the local Docker daemon using the platform default.
    ///
    /// A separate connection from `DockerRuntime`'s rather than a shared one: by
    /// the time the composition root has a runtime it is wrapped in two
    /// `InstrumentedRuntime` layers, and threading an accessor through both to
    /// reach the client would couple this to that wrapper stack. A bollard
    /// `Docker` is a connection config over a pooled transport, so a second one
    /// costs effectively nothing.
    ///
    /// Unlike `DockerRuntime::new` this does not ping: stats are a read-only
    /// diagnostic, so a daemon that is down should surface as a failed stats call
    /// and not hold up server startup.
    pub fn connect() -> Result<Self> {
        let client = Docker::connect_with_local_defaults()
            .map_err(|e| RuntimeError::BackendUnreachable(e.to_string()))?;
        Ok(Self::new(client))
    }

    /// Attach a resolver so agent containers show their registered names instead
    /// of `nasiko-agent-<uuid>`.
    pub fn with_agent_names(mut self, resolver: Arc<dyn AgentNameResolver>) -> Self {
        self.names = Some(resolver);
        self
    }

    /// One sample for one container.
    ///
    /// `one_shot` is deliberately **false**: with `one_shot: true` the daemon
    /// returns zeroed `precpu_stats`, which makes every CPU reading
    /// indeterminate. `stream: false` still returns a single frame, but one whose
    /// `precpu_stats` carries the previous read, which is what makes the delta
    /// computable.
    async fn sample(&self, name: &str) -> Option<Stats> {
        let opts = StatsOptions {
            stream: false,
            one_shot: false,
        };
        match self.client.stats(name, Some(opts)).next().await {
            Some(Ok(stats)) => Some(stats),
            Some(Err(e)) => {
                warn!(container = name, error = %e, "docker stats failed");
                None
            }
            None => None,
        }
    }
}

/// Sums the read/write halves of the blkio counters.
///
/// Docker reports one entry per device per operation, so a box with several
/// devices yields several `Read`/`Write` rows that must be added, not picked
/// from. Operation casing has varied across daemon versions, hence the
/// case-insensitive compare.
fn blkio_totals(stats: &Stats) -> (u64, u64) {
    let mut read = 0u64;
    let mut write = 0u64;
    if let Some(entries) = &stats.blkio_stats.io_service_bytes_recursive {
        for e in entries {
            match e.op.to_ascii_lowercase().as_str() {
                "read" => read = read.saturating_add(e.value),
                "write" => write = write.saturating_add(e.value),
                _ => {}
            }
        }
    }
    (read, write)
}

/// Builds one row from a container listing plus its (optional) stats sample.
///
/// `sample` is `None` for a stopped container, which still gets a row: "postgres
/// is exited" is exactly what someone opening this page needs to see. Its usage
/// fields are zero, but `cpu_percent` stays `None` so "idle" and "not reporting"
/// remain distinguishable.
fn to_container_stats(
    raw_name: &str,
    compose_service: Option<&str>,
    state: Option<&str>,
    sample: Option<&Stats>,
) -> ContainerStats {
    let name = raw_name.strip_prefix('/').unwrap_or(raw_name).to_owned();
    let group = classify(&name, compose_service);

    let (cpu, mem_used, mem_limit, rx, tx, rd, wr) = match sample {
        Some(s) => {
            let cpu = cpu_percent(
                s.cpu_stats.cpu_usage.total_usage,
                s.precpu_stats.cpu_usage.total_usage,
                s.cpu_stats.system_cpu_usage.unwrap_or(0),
                s.precpu_stats.system_cpu_usage.unwrap_or(0),
                s.cpu_stats.online_cpus.unwrap_or(0),
            );
            let (rx, tx) = network_totals(s);
            let (rd, wr) = blkio_totals(s);
            (
                cpu,
                s.memory_stats.usage.unwrap_or(0),
                s.memory_stats.limit.unwrap_or(0),
                rx,
                tx,
                rd,
                wr,
            )
        }
        None => (None, 0, 0, 0, 0, 0, 0),
    };

    ContainerStats {
        display_name: compose_service.unwrap_or(&name).to_owned(),
        name,
        group,
        state: state.unwrap_or_default().to_owned(),
        cpu_percent: cpu,
        mem_used_bytes: mem_used,
        mem_limit_bytes: mem_limit,
        net_rx_bytes: rx,
        net_tx_bytes: tx,
        block_read_bytes: rd,
        block_write_bytes: wr,
    }
}

/// Sums receive/transmit across every interface attached to the container.
fn network_totals(stats: &Stats) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    if let Some(networks) = &stats.networks {
        for n in networks.values() {
            rx = rx.saturating_add(n.rx_bytes);
            tx = tx.saturating_add(n.tx_bytes);
        }
    } else if let Some(n) = &stats.network {
        rx = n.rx_bytes;
        tx = n.tx_bytes;
    }
    (rx, tx)
}

#[async_trait]
impl ResourceStatsProvider for DockerStatsProvider {
    async fn platform_stats(&self) -> Result<PlatformStats> {
        let containers = self
            .client
            .list_containers(Some(ListContainersOptions::<String> {
                all: true,
                ..Default::default()
            }))
            .await
            .map_err(|e| RuntimeError::BackendUnreachable(e.to_string()))?;

        // Sample every container concurrently. Serially this is one round trip
        // each, and a dozen containers would push the handler into whole seconds.
        let samples = futures_util::future::join_all(containers.iter().map(|c| {
            let name = c.names.as_ref().and_then(|n| n.first()).cloned();
            async move {
                match name {
                    Some(n) => {
                        let bare = n.strip_prefix('/').unwrap_or(&n).to_owned();
                        self.sample(&bare).await
                    }
                    None => None,
                }
            }
        }))
        .await;

        let mut out: Vec<ContainerStats> = Vec::with_capacity(containers.len());
        for (c, sample) in containers.iter().zip(samples) {
            let Some(raw_name) = c.names.as_ref().and_then(|n| n.first()) else {
                continue;
            };
            let service = c
                .labels
                .as_ref()
                .and_then(|l| l.get(COMPOSE_SERVICE_LABEL))
                .map(String::as_str);
            out.push(to_container_stats(
                raw_name,
                service,
                c.state.as_deref(),
                sample.as_ref(),
            ));
        }

        if let Some(resolver) = &self.names {
            let ids: Vec<String> = out
                .iter()
                .filter_map(|c| crate::stats::agent_id_from_name(&c.name).map(str::to_owned))
                .collect();
            if !ids.is_empty() {
                let resolved = resolver.resolve(&ids).await;
                for c in &mut out {
                    if let Some(id) = crate::stats::agent_id_from_name(&c.name)
                        && let Some(display) = resolved.get(id)
                    {
                        c.display_name = display.clone();
                    }
                }
            }
        }

        let host = self.host_stats().await;
        Ok(PlatformStats {
            host,
            containers: out,
            // The Docker API exposes no host filesystem totals, so this reading
            // accounts only for Docker's own footprint. Reporting the source lets
            // the UI say so rather than render a blank gauge.
            disk_source: DiskSource::Docker,
        })
    }

    async fn agent_stats(&self, agent_id: &str) -> Result<Option<ContainerStats>> {
        // Anchored exact-match filter. An unanchored name filter is a substring
        // match in Docker, so `nasiko-agent-<id>` would also match a container
        // named `old-nasiko-agent-<id>-backup` — and this result is returned to a
        // caller authorised for exactly one agent, so a loose match here would be
        // an access-control hole, not just a cosmetic bug.
        let name = format!("nasiko-agent-{agent_id}");
        let mut filters = HashMap::new();
        filters.insert("name".to_owned(), vec![format!("^/{name}$")]);

        let found = self
            .client
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters,
                ..Default::default()
            }))
            .await
            .map_err(|e| RuntimeError::BackendUnreachable(e.to_string()))?;

        let Some(c) = found.into_iter().next() else {
            return Ok(None);
        };
        let raw_name = match c.names.as_ref().and_then(|n| n.first()) {
            Some(n) => n.clone(),
            None => format!("/{name}"),
        };

        let sample = self.sample(&name).await;
        // Agent containers carry no compose labels, so pass none and let
        // `classify` fall through to the `nasiko-agent-` prefix rule.
        let mut stats = to_container_stats(&raw_name, None, c.state.as_deref(), sample.as_ref());

        if let Some(resolver) = &self.names {
            let resolved = resolver.resolve(&[agent_id.to_owned()]).await;
            if let Some(display) = resolved.get(agent_id) {
                stats.display_name = display.clone();
            }
        }
        Ok(Some(stats))
    }
}

impl DockerStatsProvider {
    /// Host totals, best-effort: a failure here degrades one card rather than
    /// failing the whole request.
    async fn host_stats(&self) -> HostStats {
        let mut host = HostStats::default();

        match self.client.info().await {
            Ok(info) => {
                host.cpu_count = info.ncpu.unwrap_or(0) as u64;
                host.mem_total_bytes = info.mem_total.unwrap_or(0) as u64;
            }
            Err(e) => warn!(error = %e, "docker info failed"),
        }

        match self.client.df().await {
            Ok(df) => {
                if let Some(images) = &df.images {
                    host.docker_images_bytes = images
                        .iter()
                        .fold(0u64, |acc, i| acc.saturating_add(i.size.max(0) as u64));
                    // `containers` counts how many containers use the image, so 0
                    // means nothing references it — exactly what a prune reclaims.
                    // Compared against 0 and not `<= 0`: the daemon reports -1 for
                    // "not computed", and counting that as reclaimable would
                    // overstate what a prune would actually free.
                    host.docker_reclaimable_bytes = images
                        .iter()
                        .filter(|i| i.containers == 0)
                        .fold(0u64, |acc, i| acc.saturating_add(i.size.max(0) as u64));
                }
                if let Some(volumes) = &df.volumes {
                    host.docker_volumes_bytes = volumes
                        .iter()
                        .filter_map(|v| v.usage_data.as_ref())
                        .fold(0u64, |acc, u| acc.saturating_add(u.size.max(0) as u64));
                }
            }
            Err(e) => warn!(error = %e, "docker df failed"),
        }

        host
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::container::{BlkioStatsEntry, NetworkStats};

    /// Minimal `Stats` with only the fields a helper under test reads.
    fn stats_with_blkio(entries: Vec<BlkioStatsEntry>) -> Stats {
        let json = serde_json::json!({
            "read": "2026-08-09T00:00:00Z",
            "preread": "2026-08-09T00:00:00Z",
            "num_procs": 0,
            "pids_stats": {},
            "memory_stats": {},
            "blkio_stats": { "io_service_bytes_recursive": entries },
            "cpu_stats": { "cpu_usage": { "total_usage": 0, "usage_in_usermode": 0, "usage_in_kernelmode": 0 }, "throttling_data": { "periods": 0, "throttled_periods": 0, "throttled_time": 0 } },
            "precpu_stats": { "cpu_usage": { "total_usage": 0, "usage_in_usermode": 0, "usage_in_kernelmode": 0 }, "throttling_data": { "periods": 0, "throttled_periods": 0, "throttled_time": 0 } },
            "storage_stats": {},
            "name": "/x",
            "id": "x",
        });
        serde_json::from_value(json).expect("fixture parses")
    }

    fn entry(op: &str, value: u64) -> BlkioStatsEntry {
        BlkioStatsEntry {
            major: 8,
            minor: 0,
            op: op.to_owned(),
            value,
        }
    }

    #[test]
    fn blkio_sums_across_devices_not_first_match() {
        let s = stats_with_blkio(vec![
            entry("Read", 100),
            entry("Write", 10),
            entry("Read", 200),
            entry("Write", 20),
        ]);
        assert_eq!(blkio_totals(&s), (300, 30));
    }

    #[test]
    fn blkio_op_casing_is_ignored() {
        let s = stats_with_blkio(vec![entry("read", 5), entry("WRITE", 7)]);
        assert_eq!(blkio_totals(&s), (5, 7));
    }

    #[test]
    fn blkio_ignores_unrelated_ops() {
        let s = stats_with_blkio(vec![entry("Sync", 999), entry("Read", 1)]);
        assert_eq!(blkio_totals(&s), (1, 0));
    }

    #[test]
    fn blkio_absent_is_zero() {
        let s = stats_with_blkio(vec![]);
        assert_eq!(blkio_totals(&s), (0, 0));
    }

    fn net(rx: u64, tx: u64) -> NetworkStats {
        NetworkStats {
            rx_dropped: 0,
            rx_bytes: rx,
            rx_errors: 0,
            tx_packets: 0,
            tx_dropped: 0,
            rx_packets: 0,
            tx_errors: 0,
            tx_bytes: tx,
        }
    }

    #[test]
    fn network_sums_every_interface() {
        let mut s = stats_with_blkio(vec![]);
        let mut nets = HashMap::new();
        nets.insert("eth0".to_owned(), net(10, 1));
        nets.insert("eth1".to_owned(), net(20, 2));
        s.networks = Some(nets);
        assert_eq!(network_totals(&s), (30, 3));
    }

    #[test]
    fn network_falls_back_to_singular_field() {
        let mut s = stats_with_blkio(vec![]);
        s.networks = None;
        s.network = Some(net(42, 7));
        assert_eq!(network_totals(&s), (42, 7));
    }
}
