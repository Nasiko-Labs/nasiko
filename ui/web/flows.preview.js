// Flows page fixtures
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
    [{ method: "GET", path: /^\/api\/flows\?/ }, {
      data: [
        { flow_id: "fl-a1b2c3d4e5f6", title: "Help me deploy the new auth service to production", root_agent_name: "devops-agent", status: "completed", total_invocations: 4, duration_ms: 12400, created_at: "2026-06-30T10:00:00Z" },
        { flow_id: "fl-f1e2d3c4b5a6", title: "Review PR #42 for security issues and suggest fixes", root_agent_name: "coding-agent", status: "running", total_invocations: 2, duration_ms: null, created_at: "2026-06-30T15:20:00Z" },
        { flow_id: "fl-112233445566", title: "Generate documentation for the secrets module", root_agent_name: "docs-agent", status: "failed", total_invocations: 3, duration_ms: 8200, created_at: "2026-06-29T09:00:00Z" },
        { flow_id: "fl-aabbccddeeff", title: "Run integration tests on the new container runtime", root_agent_name: "qa-agent", status: "completed", total_invocations: 6, duration_ms: 45000, created_at: "2026-06-28T14:00:00Z" },
      ],
      total: 4,
    }],
  ],
};
