// Sessions page fixtures
export default {
  window: {
    fetchSessions: async () => ({
      data: [
        { session_id: "s-001", agent_name: "Coding Agent", agent_id: "a-001", last_message: "Fixed the DNS resolution issue in the container networking layer", created_at: "2026-06-30T14:22:00Z", updated_at: "2026-06-30T15:10:00Z" },
        { session_id: "s-002", agent_name: "Research Agent", agent_id: "a-002", last_message: "Here's the summary of Kubernetes operator patterns...", created_at: "2026-06-29T09:15:00Z", updated_at: "2026-06-29T10:45:00Z" },
        { session_id: "s-003", agent_name: "Docs Agent", agent_id: "a-005", last_message: "Generated API documentation for 12 endpoints", created_at: "2026-06-28T16:30:00Z", updated_at: "2026-06-28T17:00:00Z" },
        { session_id: "s-004", agent_name: "DevOps Agent", agent_id: "a-003", last_message: "Optimized the CI pipeline — build time reduced by 40%", created_at: "2026-06-27T11:45:00Z", updated_at: "2026-06-27T14:30:00Z" },
        { session_id: "s-005", agent_name: "QA Agent", agent_id: "a-004", last_message: "Created 24 integration tests covering the auth flow", created_at: "2026-06-26T08:00:00Z", updated_at: "2026-06-26T09:20:00Z" },
      ],
      total: 5,
    }),
  },
};
