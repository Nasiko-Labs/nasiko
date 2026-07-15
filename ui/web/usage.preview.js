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
    // Dates roll with "today" so the last-7-days chart always has bars.
    [{ method: "GET", path: /^\/api\/usage\/history/ }, () => {
      const counts = [210, 245, 198, 312, 278, 290, 309];
      const tokens = [86000, 96000, 76000, 122000, 107000, 108000, 102000];
      const costs = [1.52, 1.78, 1.35, 2.21, 1.95, 1.88, 1.78];
      return counts.map((request_count, i) => {
        const d = new Date();
        d.setDate(d.getDate() - (6 - i));
        return {
          date: d.toISOString().slice(0, 10),
          request_count,
          total_tokens: tokens[i],
          total_cost_usd: costs[i],
        };
      });
    }],
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
