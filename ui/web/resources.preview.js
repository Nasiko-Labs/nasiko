// Preview fixtures for resources.html. Shapes mirror
// GET /api/observability/resources (oss/server/src/observability/resources.rs).
//
// Numbers are deliberately drawn from the real cp.nasiko.dev box so the layout is
// exercised against plausible values: a 2-core / 3.7 GB host, one control-plane
// container, seven agents, and the compose infra including Tempo/Loki.

const container = (name, display, state, cpu, memMb, rxMb, txMb) => ({
  name,
  display_name: display,
  group: 'infra',
  state,
  cpu_percent: cpu,
  mem_used_bytes: Math.round(memMb * 1024 * 1024),
  mem_limit_bytes: Math.round(3.7 * 1024 * 1024 * 1024),
  net_rx_bytes: Math.round(rxMb * 1024 * 1024),
  net_tx_bytes: Math.round(txMb * 1024 * 1024),
  block_read_bytes: 0,
  block_write_bytes: 0,
});

const payload = {
  data: {
    host: {
      cpu_count: 2,
      mem_total_bytes: Math.round(3.7 * 1024 * 1024 * 1024),
      docker_images_bytes: 3_231_000_000,
      docker_volumes_bytes: 72_070_000,
      docker_reclaimable_bytes: 1_216_000_000,
      disk_total_bytes: null,
      disk_used_bytes: null,
    },
    groups: {
      control_plane: [container('nasiko-server-1', 'server', 'running', 34.2, 412, 180, 96)],
      agent_runtime: [
        container('nasiko-agent-6e05532a', 'finance-agent', 'running', 96.4, 268, 22, 14),
        container('nasiko-agent-a7f18d01', 'nutrition', 'running', 12.8, 194, 12, 8),
        container('nasiko-agent-e1685913', 'docs', 'running', 4.1, 176, 9, 5),
        container('nasiko-agent-a8b2edc1', 'hr-agent', 'running', 1.2, 168, 7, 4),
        container('nasiko-agent-509b9de7', 'devops-agent', 'running', 0.6, 165, 6, 3),
        // cpu_percent null exercises the "not reporting" path — must not render 0%.
        container('nasiko-agent-c0a2e3f3', 'legal-agent', 'running', null, 161, 5, 3),
        container('nasiko-agent-446ee8d3', 'infra-agent', 'exited', null, 0, 0, 0),
      ],
      infra: [
        container('nasiko-postgres-1', 'postgres', 'running', 8.4, 268, 340, 210),
        container('nasiko-tempo-1', 'tempo', 'running', 6.2, 152, 44, 12),
        container('nasiko-loki-1', 'loki', 'running', 5.1, 148, 38, 11),
        container('nasiko-otel-collector-1', 'otel-collector', 'running', 3.3, 96, 52, 49),
        container('nasiko-redis-1', 'redis', 'running', 1.1, 12, 28, 26),
        container('nasiko-rustfs-1', 'rustfs', 'running', 0.8, 88, 18, 22),
        container('nasiko-caddy-1', 'caddy', 'running', 0.4, 24, 96, 104),
      ],
    },
    disk_source: 'docker',
    collected_at: '2026-08-09T11:20:00Z',
  },
};

export default {
  fetch: [['GET /api/observability/resources', payload]],
  window: {
    fetchResourceStats: async () => payload,
  },
  scenarios: {
    // The Kubernetes / simulated-runtime answer: the endpoint 503s and the page
    // must explain itself rather than render a box that looks idle.
    // No page.reload() here: a reload reinstalls the fixtures and throws the
    // override away. The page polls every 5s, so swapping the helper and waiting
    // for the next tick is what actually exercises the failure path.
    unavailable: async (page) => {
      await page.evaluate(() => {
        window.fetchResourceStats = async () => {
          throw new Error("resource stats are not available for the 'kubernetes' runtime");
        };
      });
      await page.waitForSelector('app-empty-state', { timeout: 15000 });
    },
  },
};
