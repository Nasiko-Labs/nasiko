// Sessions page fixtures.
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

export default {
  fetch: [
    // Cursor-paginated shape, matching CursorPage from oss/server/src/chat/routes.rs.
    // `next_cursor` is set so the pager's "Load more" state is exercised.
    [{ method: "GET", path: /^\/api\/chat\/sessions/ }, {
      data: sessionsData,
      has_more: true,
      next_cursor: "preview-cursor-page-2",
      prev_cursor: null,
    }],
    [{ method: "DELETE", path: /^\/api\/chat\/sessions\// }, { ok: true }],
  ],
  window: {
    deleteSession: async () => {},
  },
  scenarios: {
    empty: async (page) => {
      await page.evaluate(() => {
        window.fetchSessions = async () => ({ data: [], total: 0 });
      });
      await page.evaluate(() => {
        document.querySelector("sessions-page").remove();
        const el = document.createElement("sessions-page");
        document.body.appendChild(el);
      });
      await page.waitForSelector("app-empty-state");
    },
  },
};
