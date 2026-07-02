// Usage page fixtures — matches backend /usage/* response shapes
export default {
  fetch: [
    ["GET /api/usage/summary", {
      request_count: 1842,
      total_input_tokens: 482000,
      total_output_tokens: 215000,
      total_tokens: 697000,
      total_cost_usd: 12.47,
      avg_latency_ms: 342.5,
      period_days: 30,
    }],
    [{ method: "GET", path: /^\/api\/usage\/history/ }, [
      { date: "2026-06-24", request_count: 210, total_tokens: 86000, total_cost_usd: 1.52 },
      { date: "2026-06-25", request_count: 245, total_tokens: 96000, total_cost_usd: 1.78 },
      { date: "2026-06-26", request_count: 198, total_tokens: 76000, total_cost_usd: 1.35 },
      { date: "2026-06-27", request_count: 312, total_tokens: 122000, total_cost_usd: 2.21 },
      { date: "2026-06-28", request_count: 278, total_tokens: 107000, total_cost_usd: 1.95 },
      { date: "2026-06-29", request_count: 290, total_tokens: 108000, total_cost_usd: 1.88 },
      { date: "2026-06-30", request_count: 309, total_tokens: 102000, total_cost_usd: 1.78 },
    ]],
    [{ method: "GET", path: /^\/api\/usage\/by-agent/ }, {
      data: [
        { agent_name: "coding-agent", request_count: 842, total_input_tokens: 220000, total_output_tokens: 98000, total_tokens: 318000, total_cost_usd: 5.62, avg_latency_ms: 380 },
        { agent_name: "research-agent", request_count: 512, total_input_tokens: 145000, total_output_tokens: 62000, total_tokens: 207000, total_cost_usd: 3.55, avg_latency_ms: 290 },
        { agent_name: "qa-agent", request_count: 310, total_input_tokens: 78000, total_output_tokens: 35000, total_tokens: 113000, total_cost_usd: 2.10, avg_latency_ms: 420 },
        { agent_name: "devops-agent", request_count: 120, total_input_tokens: 28000, total_output_tokens: 14000, total_tokens: 42000, total_cost_usd: 0.82, avg_latency_ms: 310 },
        { agent_name: "docs-agent", request_count: 58, total_input_tokens: 11000, total_output_tokens: 6000, total_tokens: 17000, total_cost_usd: 0.38, avg_latency_ms: 250 },
      ],
      total: 5,
    }],
    [{ method: "GET", path: /^\/api\/usage\/by-model/ }, {
      data: [
        { provider: "anthropic", model: "claude-sonnet-4-6", request_count: 1200, total_input_tokens: 350000, total_output_tokens: 150000, total_tokens: 500000, total_cost_usd: 8.20, avg_latency_ms: 340 },
        { provider: "anthropic", model: "claude-haiku-4-5", request_count: 520, total_input_tokens: 100000, total_output_tokens: 50000, total_tokens: 150000, total_cost_usd: 2.10, avg_latency_ms: 180 },
        { provider: "openai", model: "gpt-4o", request_count: 122, total_input_tokens: 32000, total_output_tokens: 15000, total_tokens: 47000, total_cost_usd: 2.17, avg_latency_ms: 450 },
      ],
      total: 3,
    }],
  ],
};
