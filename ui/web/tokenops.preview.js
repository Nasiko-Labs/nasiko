// TokenOps fixtures — matches GET /api/observability/finops/dashboard
// (FinopsDashboardResponse in oss/server/src/observability/service.rs; see /api/docs).
export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/observability\/finops\/dashboard/ }, {
      data: {
        summary: {
          total_cost: 12.473,
          total_operations: 1842,
          operations_last_24h: 214,
          average_cost: 0.007,
          active_agents: 5,
          total_agents: 7,
          total_container_hours: 214.8,
        },
        agents: [
          { agent_id: "a-001", agent_name: "coding-agent", total_cost: 5.62, operations: 842, avg_cost_per_operation: 0.0067, prompt_tokens: 220000, completion_tokens: 98000, cache_read_tokens: 64000, cache_creation_tokens: 21000, total_tokens: 318000, avg_latency_ms: 380, version: "v4", container_hours: 96.2 },
          { agent_id: "a-002", agent_name: "research-agent", total_cost: 3.55, operations: 512, avg_cost_per_operation: 0.0069, prompt_tokens: 145000, completion_tokens: 62000, cache_read_tokens: 38000, cache_creation_tokens: 12000, total_tokens: 207000, avg_latency_ms: 290, version: "v2", container_hours: 54.1 },
          { agent_id: "a-004", agent_name: "qa-agent", total_cost: 2.10, operations: 310, avg_cost_per_operation: 0.0068, prompt_tokens: 78000, completion_tokens: 35000, cache_read_tokens: 9000, cache_creation_tokens: 4000, total_tokens: 113000, avg_latency_ms: 420, version: "v7", container_hours: 31.5 },
          { agent_id: "a-003", agent_name: "devops-agent", total_cost: 0.82, operations: 120, avg_cost_per_operation: 0.0068, prompt_tokens: 28000, completion_tokens: 14000, cache_read_tokens: 0, cache_creation_tokens: 0, total_tokens: 42000, avg_latency_ms: 310, version: "v1", container_hours: 18.0 },
          { agent_id: "a-005", agent_name: "docs-agent", total_cost: 0.38, operations: 58, avg_cost_per_operation: 0.0066, prompt_tokens: 11000, completion_tokens: 6000, cache_read_tokens: 2000, cache_creation_tokens: 800, total_tokens: 17000, avg_latency_ms: 250, version: "v3", container_hours: 12.4 },
          { agent_id: "a-006", agent_name: "finance-bot", total_cost: 0, operations: 0, avg_cost_per_operation: 0, prompt_tokens: 0, completion_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0, total_tokens: 0, avg_latency_ms: null, version: null, container_hours: 2.6 },
        ],
        token_usage: {
          total_tokens: 697000,
          prompt_tokens: 482000,
          completion_tokens: 215000,
          cache_read_tokens: 113000,
          cache_creation_tokens: 37800,
          avg_tokens_per_operation: 378,
        },
      },
      status_code: 200,
      message: "FinOps dashboard retrieved successfully",
    }],
  ],
};
