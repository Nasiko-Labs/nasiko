// Sessions page fixtures
const sessionsData = [
  { session_id: "s-001", agent_name: "Coding Agent", agent_id: "a-001", last_message: "Fixed the DNS resolution issue in the container networking layer", message_count: 12, created_at: "2026-07-03T14:22:00Z", updated_at: "2026-07-03T15:10:00Z" },
  { session_id: "s-002", agent_name: "Research Agent", agent_id: "a-002", last_message: "Here's the summary of Kubernetes operator patterns and best practices for implementing CRDs", message_count: 8, created_at: "2026-07-03T09:15:00Z", updated_at: "2026-07-03T10:45:00Z" },
  { session_id: "s-003", agent_name: "Docs Agent", agent_id: "a-005", last_message: "Generated API documentation for 12 endpoints including request/response schemas", message_count: 5, created_at: "2026-07-02T16:30:00Z", updated_at: "2026-07-02T17:00:00Z" },
  { session_id: "s-004", agent_name: "DevOps Agent", agent_id: "a-003", last_message: "Optimized the CI pipeline — build time reduced by 40% after parallelizing test stages", message_count: 22, created_at: "2026-06-30T11:45:00Z", updated_at: "2026-06-30T14:30:00Z" },
  { session_id: "s-005", agent_name: "QA Agent", agent_id: "a-004", last_message: "Created 24 integration tests covering the auth flow and token refresh edge cases", message_count: 15, created_at: "2026-06-25T08:00:00Z", updated_at: "2026-06-25T09:20:00Z" },
  { session_id: "s-006", agent_name: "Coding Agent", agent_id: "a-001", last_message: "Refactored the routing engine to use a trait-based design for better testability", message_count: 31, created_at: "2026-06-20T10:00:00Z", updated_at: "2026-06-20T12:00:00Z" },
];

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/observability\/session\/list/ }, {
      data: {
        sessions: [
          { session_id: "s-001", first_input: "Fix the DNS resolution issue in container networking", num_traces: 4, token_usage: { total: 18400 }, trace_latency_ms_p50: 1240 },
          { session_id: "s-002", first_input: "Summarize Kubernetes operator patterns", num_traces: 2, token_usage: { total: 7200 }, trace_latency_ms_p50: 890 },
          { session_id: "s-004", first_input: "Optimize the CI pipeline build times", num_traces: 9, token_usage: { total: 52300 }, trace_latency_ms_p50: 2150 },
          { session_id: "s-006", first_input: "Refactor the routing engine to a trait-based design", num_traces: 12, token_usage: { total: 121000 }, trace_latency_ms_p50: 3400 },
        ],
      },
    }],
    [{ method: "GET", path: /^\/api\/chat\/sessions/ }, { data: sessionsData, total: sessionsData.length }],
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
