// Observability module fixtures and scenarios. One page, three views (history,
// flows, resources) switched by the nested sidebar — so this file has to stub
// every endpoint any of them calls, and each view gets a `view-*` scenario.
//
// The stats columns come from the session rows themselves — `trace_count`,
// `total_tokens` and `latency_p50_ms` are aggregated by the list query (see
// SESSION_LIST_SELECT in oss/server/src/chat/routes.rs), not fetched from the
// trace store. s-003 and s-005 leave `total_tokens` null on purpose: that is
// the BYO-key agent case, where nothing was platform-paid and the cell reads
// "—" while traces and latency are still present.
const sessionsData = [
  { session_id: "s-001", agent_name: "Coding Agent", agent_id: "a-001", last_message: "Fixed the DNS resolution issue in the container networking layer", message_count: 12, trace_count: 4, total_tokens: 18400, latency_p50_ms: 1240, created_at: "2026-07-03T14:22:00Z", updated_at: "2026-07-03T15:10:00Z" },
  { session_id: "s-002", agent_name: "Research Agent", agent_id: "a-002", last_message: "Here's the summary of Kubernetes operator patterns and best practices for implementing CRDs", message_count: 8, trace_count: 2, total_tokens: 7200, latency_p50_ms: 890, created_at: "2026-07-03T09:15:00Z", updated_at: "2026-07-03T10:45:00Z" },
  { session_id: "s-003", agent_name: "Docs Agent", agent_id: "a-005", last_message: "Generated API documentation for 12 endpoints including request/response schemas", message_count: 5, trace_count: 3, total_tokens: null, latency_p50_ms: 640, created_at: "2026-07-02T16:30:00Z", updated_at: "2026-07-02T17:00:00Z" },
  { session_id: "s-004", agent_name: "DevOps Agent", agent_id: "a-003", last_message: "Optimized the CI pipeline — build time reduced by 40% after parallelizing test stages", message_count: 22, trace_count: 9, total_tokens: 52300, latency_p50_ms: 2150, created_at: "2026-06-30T11:45:00Z", updated_at: "2026-06-30T14:30:00Z" },
  { session_id: "s-005", agent_name: "QA Agent", agent_id: "a-004", last_message: "Created 24 integration tests covering the auth flow and token refresh edge cases", message_count: 15, trace_count: 6, total_tokens: null, latency_p50_ms: 1810, created_at: "2026-06-25T08:00:00Z", updated_at: "2026-06-25T09:20:00Z" },
  { session_id: "s-006", agent_name: "Coding Agent", agent_id: "a-001", last_message: "Refactored the routing engine to use a trait-based design for better testability", message_count: 31, trace_count: 12, total_tokens: 121000, latency_p50_ms: 3400, created_at: "2026-06-20T10:00:00Z", updated_at: "2026-06-20T12:00:00Z" },
];

// resources view: shapes mirror GET /api/observability/resources
// (oss/server/src/observability/resources.rs). Numbers are deliberately drawn
// from the real cp.nasiko.dev box so the layout is exercised against plausible
// values: a 2-core / 3.7 GB host, one control-plane container, seven agents, and
// the compose infra including Tempo/Loki.
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

const resources = {
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
  fetch: [
    // history view — cursor-paginated shape, matching CursorPage from
    // oss/server/src/chat/routes.rs. `next_cursor` is set so the pager's
    // "Load more" state is exercised.
    [{ method: "GET", path: /^\/api\/chat\/sessions/ }, {
      data: sessionsData,
      has_more: true,
      next_cursor: "preview-cursor-page-2",
      prev_cursor: null,
    }],
    [{ method: "DELETE", path: /^\/api\/chat\/sessions\// }, { ok: true }],
    // flows view
    [{ method: "GET", path: /^\/api\/flows\?/ }, {
      data: [
        { flow_id: "fl-a1b2c3d4e5f6", title: "Help me deploy the new auth service to production", root_agent_name: "devops-agent", status: "completed", total_invocations: 4, duration_ms: 12400, created_at: "2026-06-30T10:00:00Z" },
        { flow_id: "fl-f1e2d3c4b5a6", title: "Review PR #42 for security issues and suggest fixes", root_agent_name: "coding-agent", status: "running", total_invocations: 2, duration_ms: null, created_at: "2026-06-30T15:20:00Z" },
        { flow_id: "fl-112233445566", title: "Generate documentation for the secrets module", root_agent_name: "docs-agent", status: "failed", total_invocations: 3, duration_ms: 8200, created_at: "2026-06-29T09:00:00Z" },
        { flow_id: "fl-aabbccddeeff", title: "Run integration tests on the new container runtime", root_agent_name: "qa-agent", status: "completed", total_invocations: 6, duration_ms: 45000, created_at: "2026-06-28T14:00:00Z" },
      ],
      total: 4,
    }],
    // resources view
    ['GET /api/observability/resources', resources],
  ],
  window: {
    deleteSession: async () => {},
    fetchResourceStats: async () => resources,
  },
  scenarios: {
    // ── The module's views ──────────────────────────────────────────────────
    // Each opens the way a shared link does (`?view=`), which also proves the
    // shell honours the param on load rather than only on a nav click.
    "view-flows": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=flows`);
      await page.waitForSelector("flows-page .cell-agent");
      await page.waitForTimeout(300);
    },
    "view-resources": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=resources`);
      await page.waitForSelector("resources-page .pane-title");
      await page.waitForTimeout(300);
    },
    // Switching by clicking the nested sidebar: the sidebar itself must not
    // change between this and the default capture.
    "view-switch-click": async (page) => {
      await page.waitForSelector(".sessions-table");
      await page.click('app-module-nav [data-section="flows"]');
      await page.waitForSelector("flows-page .cell-agent");
      await page.waitForTimeout(400);
    },
    // The Kubernetes / simulated-runtime answer for the resources view: the
    // endpoint 503s and the page must explain itself rather than render a box
    // that looks idle. No page.reload() after the override — a reload
    // reinstalls the fixtures and throws it away. The view polls every 5s, so
    // swapping the helper and waiting for the next tick is what actually
    // exercises the failure path.
    "resources-unavailable": async (page) => {
      await page.goto(`${page.url().split("?")[0]}?view=resources`);
      await page.waitForSelector("resources-page .pane-title");
      await page.evaluate(() => {
        window.fetchResourceStats = async () => {
          throw new Error("resource stats are not available for the 'kubernetes' runtime");
        };
      });
      await page.waitForSelector("resources-page app-empty-state", { timeout: 15000 });
    },
    empty: async (page) => {
      await page.evaluate(() => {
        window.fetchSessions = async () => ({ data: [], total: 0 });
        // Re-created inside the shell, not on <body>: the shell is the content
        // card and owns the nav gutter, so a page element parked next to it
        // would render with neither.
        const shell = document.querySelector("module-shell");
        shell.querySelector("sessions-page").remove();
        const el = document.createElement("sessions-page");
        el.dataset.view = "history";
        el.dataset.title = "Execution history";
        shell.append(el);
      });
      await page.waitForSelector("app-empty-state");
    },
  },
};
